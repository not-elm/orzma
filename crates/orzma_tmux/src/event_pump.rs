//! Draining, logging, and routing of tmux transport events into the global
//! projection events the observers consume.

use crate::components::WindowFlags;
use crate::enumerate::{WINDOW_FLAGS_SUBSCRIPTION, parse_window_rows};
use crate::events::{
    TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxLayoutChanged, TmuxSessionChanged,
    TmuxWindowAdded, TmuxWindowClosed, TmuxWindowFlagsChanged, TmuxWindowRenamed,
    TmuxWindowsRetained,
};
use bevy::prelude::Commands;
use tmux_control::{ClientEvent, ControlEvent, TransportEvent};
use tmux_control_parser::{PaneId, SessionId, WindowId};

/// Returns the first non-empty trimmed output line of a completed command, or
/// `None` when the command failed (logged with the `what` label) or the output
/// is blank. Pure: the caller owns the `CommandId` correlation.
pub(crate) fn first_reply_line(ok: bool, output: &[String], what: &str) -> Option<String> {
    if !ok {
        tracing::warn!("{what} query command failed");
        return None;
    }
    output
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Returns the new session id if `events` contains a session-change to an id
/// different from `current`, i.e. a real `switch-client`. Returns `None` on the
/// first attach (`current == None`) or when the id is unchanged, so the initial
/// enumeration is not duplicated and only an actual switch triggers a rebuild.
///
/// `%session-changed` and `%session-renamed` are always treated as a switch.
/// `%client-session-changed` is only treated as a switch when its `client`
/// field equals `own_client`; if `own_client` is `None` (not yet known),
/// `%client-session-changed` is ignored to avoid spurious teardown from
/// foreign-client events arriving before the own client name is resolved.
///
/// The switch decision lives here (driven from the per-frame drain) rather than
/// in the `on_session_changed` observer, because the teardown + re-enumeration
/// it triggers need the event batch and the live `NonSend` client, which an
/// observer cannot access.
pub(crate) fn detect_session_switch(
    events: &[TransportEvent],
    current: Option<SessionId>,
    own_client: Option<&str>,
) -> Option<SessionId> {
    let current = current?;
    for event in events {
        let next = match event {
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::SessionChanged {
                session,
                ..
            })) => *session,
            TransportEvent::Protocol(ClientEvent::Notification(
                ControlEvent::ClientSessionChanged {
                    client, session, ..
                },
            )) => {
                if own_client == Some(client.as_str()) {
                    *session
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        if next != current {
            return Some(next);
        }
    }
    None
}

/// True when the batch contains a `%session-window-changed` for `current` — the
/// own session's current window changed (`next-window` / `previous-window` /
/// `select-window`). tmux broadcasts `%session-window-changed` to every control
/// client server-wide, so a foreign session's switch (or `current == None`)
/// returns false and does not trigger a needless active-pane re-query.
///
/// NOTE: tmux emits *only* `%session-window-changed` for such a switch, never a
/// `%window-pane-changed`, so the caller must re-query the active pane
/// (`ActivePane`) to move `ActiveWindow`/`ActivePane`. Without that the
/// switch never reaches the projection and the UI stays on the old window.
pub(crate) fn detect_window_switch(events: &[TransportEvent], current: Option<SessionId>) -> bool {
    let Some(current) = current else {
        return false;
    };
    events.iter().any(|event| {
        matches!(
            event,
            TransportEvent::Protocol(ClientEvent::Notification(
                ControlEvent::SessionWindowChanged { session, .. }
            )) if *session == current
        )
    })
}

/// True when the batch contains a `%window-add` — a window was created
/// (`new-window`).
///
/// NOTE: tmux does NOT emit a `%layout-change` for a freshly added window
/// (verified against tmux 3.6a: `new-window` sends only `%window-add` +
/// `%session-window-changed` + `%output`), so the new window's pane layout
/// never arrives via notifications. The caller must re-enumerate
/// (`list-windows`) to fetch the layout and project the pane; without it the
/// new window has no pane entity and renders black.
pub(crate) fn detect_window_added(events: &[TransportEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd { .. }))
        )
    })
}

/// Parses an `@N %M` line into `(WindowId, PaneId)`.
pub(crate) fn parse_active_pane(line: &str) -> Option<(WindowId, PaneId)> {
    let mut parts = line.split_whitespace();
    let window = parts.next()?.strip_prefix('@')?.parse().ok()?;
    let pane = parts.next()?.strip_prefix('%')?.parse().ok()?;
    Some((WindowId(window), PaneId(pane)))
}

/// Triggers the projection event a single tmux notification maps to (session,
/// window, pane, layout, flags). `own_client` gates `%client-session-changed`.
pub(crate) fn trigger_notification(
    commands: &mut Commands,
    own_client: Option<&str>,
    event: &ControlEvent,
) {
    match event {
        ControlEvent::SessionChanged { session, name }
        | ControlEvent::SessionRenamed { session, name } => {
            commands.trigger(TmuxSessionChanged {
                session: *session,
                name: name.clone(),
            });
        }
        ControlEvent::ClientSessionChanged {
            client,
            session,
            name,
        } if own_client == Some(client.as_str()) => {
            commands.trigger(TmuxSessionChanged {
                session: *session,
                name: name.clone(),
            });
        }
        ControlEvent::WindowAdd { window } => {
            commands.trigger(TmuxWindowAdded {
                window: *window,
                index: 0,
                name: String::new(),
            });
        }
        ControlEvent::WindowClose { window } | ControlEvent::UnlinkedWindowClose { window } => {
            commands.trigger(TmuxWindowClosed { window: *window });
        }
        ControlEvent::WindowRenamed { window, name } => {
            commands.trigger(TmuxWindowRenamed {
                window: *window,
                name: name.clone(),
            });
        }
        ControlEvent::LayoutChange {
            window,
            visible_layout,
            ..
        } => {
            commands.trigger(TmuxLayoutChanged {
                window: *window,
                layout: visible_layout.clone(),
            });
        }
        ControlEvent::WindowPaneChanged { window, pane } => {
            commands.trigger(TmuxActivePaneChanged {
                window: *window,
                pane: *pane,
                from_notification: true,
            });
        }
        ControlEvent::SubscriptionChanged {
            name,
            window: Some(window),
            value,
            ..
        } if name == WINDOW_FLAGS_SUBSCRIPTION => {
            commands.trigger(TmuxWindowFlagsChanged {
                window: *window,
                flags: WindowFlags::parse(value),
            });
        }
        _ => {}
    }
}

/// Decomposes a `list-windows` reply into per-row `TmuxWindowAdded` +
/// `TmuxWindowFlagsChanged` + `TmuxLayoutChanged` (+ `TmuxActiveWindowChanged`
/// for the active row), then one `TmuxWindowsRetained` prune.
pub(crate) fn trigger_seed(commands: &mut Commands, output: &[String]) {
    let rows = match parse_window_rows(output) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "failed to parse list-windows reply");
            return;
        }
    };
    let mut ids = Vec::with_capacity(rows.len());
    for row in &rows {
        commands.trigger(TmuxWindowAdded {
            window: row.id,
            index: row.index,
            name: row.name.clone(),
        });
        commands.trigger(TmuxWindowFlagsChanged {
            window: row.id,
            flags: row.flags,
        });
        commands.trigger(TmuxLayoutChanged {
            window: row.id,
            layout: row.layout.clone(),
        });
        if row.active {
            commands.trigger(TmuxActiveWindowChanged { window: row.id });
        }
        ids.push(row.id);
    }
    commands.trigger(TmuxWindowsRetained { windows: ids });
}

/// Emits a `tracing` line describing a single transport event.
pub(crate) fn log_transport_event(event: &TransportEvent) {
    match event {
        TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, .. }) => {
            tracing::debug!(?id, ok, "tmux command complete");
        }
        TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
            tracing::debug!(?notification, "tmux notification");
        }
        TransportEvent::Closed { reason } => {
            tracing::info!(reason, "tmux transport closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use tmux_control::ControlEvent;
    use tmux_control_parser::{SessionId, WindowId};

    #[test]
    fn first_reply_line_returns_first_non_empty_trimmed() {
        let output = vec!["".to_string(), "  3.6a  ".to_string()];
        assert_eq!(
            first_reply_line(true, &output, "version"),
            Some("3.6a".to_string())
        );
    }

    #[test]
    fn first_reply_line_returns_none_on_failure() {
        assert_eq!(first_reply_line(false, &["x".to_string()], "version"), None);
    }

    #[test]
    fn first_reply_line_returns_none_on_blank_output() {
        assert_eq!(
            first_reply_line(true, &["   ".to_string()], "version"),
            None
        );
    }

    #[test]
    fn parse_active_pane_parses_window_and_pane() {
        assert_eq!(parse_active_pane("@7 %88"), Some((WindowId(7), PaneId(88))));
        assert_eq!(parse_active_pane("garbage"), None);
    }

    #[test]
    fn client_session_changed_triggers_session_changed() {
        use crate::events::TmuxSessionChanged;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::SessionId;

        #[derive(Resource, Default, Clone)]
        struct Seen(Arc<Mutex<Vec<(u32, String)>>>);

        let mut app = App::new();
        app.init_resource::<Seen>();
        app.add_observer(|ev: On<TmuxSessionChanged>, seen: Res<Seen>| {
            seen.0.lock().unwrap().push((ev.session.0, ev.name.clone()));
        });
        app.add_systems(Update, |mut commands: Commands| {
            trigger_notification(
                &mut commands,
                Some("main"),
                &ControlEvent::ClientSessionChanged {
                    client: "main".to_string(),
                    session: SessionId(9),
                    name: "beta".to_string(),
                },
            );
        });
        let seen = app.world().resource::<Seen>().clone();
        app.update();

        assert_eq!(*seen.0.lock().unwrap(), vec![(9, "beta".to_string())]);
    }

    fn client_session_changed(client: &str, session: SessionId) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::ClientSessionChanged {
                client: client.to_string(),
                session,
                name: "s".to_string(),
            },
        ))
    }

    fn session_changed(session: SessionId) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::SessionChanged {
            session,
            name: "s".to_string(),
        }))
    }

    #[test]
    fn foreign_client_session_changed_is_ignored() {
        let events = vec![client_session_changed("other-client", SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), Some("orzma-0")),
            None
        );
    }

    #[test]
    fn own_client_session_changed_is_a_switch() {
        let events = vec![client_session_changed("orzma-0", SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), Some("orzma-0")),
            Some(SessionId(9))
        );
    }

    #[test]
    fn client_session_changed_ignored_when_own_name_unknown() {
        let events = vec![client_session_changed("orzma-0", SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), None),
            None
        );
    }

    #[test]
    fn plain_session_changed_is_a_switch_regardless_of_name() {
        let events = vec![session_changed(SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), None),
            Some(SessionId(9))
        );
    }

    #[test]
    fn detect_session_switch_reports_new_id_only_on_change() {
        use tmux_control_parser::SessionId;
        let changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionChanged {
                session: SessionId(2),
                name: "b".to_string(),
            },
        ))];
        assert_eq!(detect_session_switch(&changed, None, None), None);
        assert_eq!(
            detect_session_switch(&changed, Some(SessionId(2)), None),
            None
        );
        assert_eq!(
            detect_session_switch(&changed, Some(SessionId(1)), None),
            Some(SessionId(2))
        );
        assert_eq!(detect_session_switch(&[], Some(SessionId(1)), None), None);

        let client_changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::ClientSessionChanged {
                client: "main".to_string(),
                session: SessionId(3),
                name: "c".to_string(),
            },
        ))];
        assert_eq!(
            detect_session_switch(&client_changed, Some(SessionId(1)), Some("main")),
            Some(SessionId(3))
        );
        assert_eq!(
            detect_session_switch(&client_changed, Some(SessionId(3)), Some("main")),
            None
        );
    }

    #[test]
    fn detect_window_switch_flags_own_session_window_changed() {
        use tmux_control_parser::{SessionId, WindowId};
        let switched = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionWindowChanged {
                session: SessionId(1),
                window: WindowId(4),
            },
        ))];
        // Own session's window switch is detected.
        assert!(detect_window_switch(&switched, Some(SessionId(1))));
        // A foreign session's broadcast %session-window-changed is ignored.
        assert!(!detect_window_switch(&switched, Some(SessionId(2))));
        // No current session yet → nothing to switch.
        assert!(!detect_window_switch(&switched, None));
        assert!(!detect_window_switch(&[], Some(SessionId(1))));

        let session_changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionChanged {
                session: SessionId(2),
                name: "b".to_string(),
            },
        ))];
        assert!(!detect_window_switch(&session_changed, Some(SessionId(2))));
    }

    #[test]
    fn detect_window_added_flags_window_add() {
        use tmux_control_parser::WindowId;
        let added = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::WindowAdd {
                window: WindowId(7),
            },
        ))];
        assert!(detect_window_added(&added));
        assert!(!detect_window_added(&[]));

        let closed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::WindowClose {
                window: WindowId(7),
            },
        ))];
        assert!(!detect_window_added(&closed));
    }

    #[test]
    fn unlinked_window_close_triggers_window_closed() {
        use crate::events::TmuxWindowClosed;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::WindowId;

        #[derive(Resource, Clone)]
        struct Sink(Arc<Mutex<Vec<WindowId>>>);

        let mut app = App::new();
        let sink = Sink(Arc::new(Mutex::new(Vec::new())));
        app.insert_resource(sink.clone());
        app.add_observer(|ev: On<TmuxWindowClosed>, sink: Res<Sink>| {
            sink.0.lock().unwrap().push(ev.window);
        });
        app.add_systems(Update, |mut commands: Commands| {
            trigger_notification(
                &mut commands,
                None,
                &ControlEvent::UnlinkedWindowClose {
                    window: WindowId(3),
                },
            );
        });
        app.update();

        assert_eq!(*sink.0.lock().unwrap(), vec![WindowId(3)]);
    }

    #[test]
    fn seed_reply_triggers_per_row_events_then_retain() {
        use crate::events::{TmuxLayoutChanged, TmuxWindowAdded, TmuxWindowsRetained};
        use std::sync::{Arc, Mutex};

        #[derive(Resource, Default, Clone)]
        struct Log(Arc<Mutex<Vec<String>>>);

        let mut app = App::new();
        app.init_resource::<Log>();
        app.add_observer(|ev: On<TmuxWindowAdded>, log: Res<Log>| {
            log.0.lock().unwrap().push(format!("add@{}", ev.window.0));
        });
        app.add_observer(|ev: On<TmuxLayoutChanged>, log: Res<Log>| {
            log.0
                .lock()
                .unwrap()
                .push(format!("layout@{}", ev.window.0));
        });
        app.add_observer(|ev: On<TmuxWindowsRetained>, log: Res<Log>| {
            log.0
                .lock()
                .unwrap()
                .push(format!("retain{}", ev.windows.len()));
        });
        app.add_systems(Update, |mut commands: Commands| {
            trigger_seed(
                &mut commands,
                &["1\t@1\t0\tabcd,80x24,0,0,5\t0000,80x24,0,0,5\t\tmain".to_string()],
            );
        });

        let log = app.world().resource::<Log>().clone();
        app.update();

        assert_eq!(*log.0.lock().unwrap(), vec!["add@1", "layout@1", "retain1"]);
    }

    #[test]
    fn session_renamed_maps_to_session_changed() {
        use crate::events::TmuxSessionChanged;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::SessionId;

        #[derive(Resource, Default, Clone)]
        struct Captured(Arc<Mutex<Vec<(u32, String)>>>);

        let mut app = App::new();
        app.init_resource::<Captured>();
        app.add_observer(|ev: On<TmuxSessionChanged>, captured: Res<Captured>| {
            captured
                .0
                .lock()
                .unwrap()
                .push((ev.session.0, ev.name.clone()));
        });
        app.add_systems(Update, |mut commands: Commands| {
            trigger_notification(
                &mut commands,
                None,
                &ControlEvent::SessionRenamed {
                    session: SessionId(1),
                    name: "renamed".to_string(),
                },
            );
        });

        let captured = app.world().resource::<Captured>().clone();
        app.update();

        assert_eq!(
            *captured.0.lock().unwrap(),
            vec![(1, "renamed".to_string())]
        );
    }

    #[test]
    fn window_flags_subscription_triggers_flags_changed() {
        use crate::components::WindowFlags;
        use crate::enumerate::WINDOW_FLAGS_SUBSCRIPTION;
        use crate::events::TmuxWindowFlagsChanged;
        use std::sync::{Arc, Mutex};

        #[derive(Resource, Default, Clone)]
        struct Captured(Arc<Mutex<Vec<(WindowId, WindowFlags)>>>);

        #[derive(Resource)]
        struct Notification(ControlEvent);

        let line = format!("%subscription-changed {WINDOW_FLAGS_SUBSCRIPTION} $1 @2 0 - : *Z");
        let notification = ControlEvent::parse(line.as_bytes()).unwrap();

        let mut app = App::new();
        app.init_resource::<Captured>();
        app.insert_resource(Notification(notification));
        app.add_observer(|ev: On<TmuxWindowFlagsChanged>, captured: Res<Captured>| {
            captured.0.lock().unwrap().push((ev.window, ev.flags));
        });
        app.add_systems(
            Update,
            |mut commands: Commands, notification: Res<Notification>| {
                trigger_notification(&mut commands, None, &notification.0);
            },
        );

        let captured = app.world().resource::<Captured>().clone();
        app.update();

        assert_eq!(
            *captured.0.lock().unwrap(),
            vec![(WindowId(2), WindowFlags::ZOOM)]
        );
    }
}
