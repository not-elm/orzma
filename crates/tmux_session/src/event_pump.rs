//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the global projection events the observers consume.

use crate::enumerate::parse_window_rows;
use crate::events::{
    TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxLayoutChanged, TmuxSessionChanged,
    TmuxWindowAdded, TmuxWindowClosed, TmuxWindowRenamed, TmuxWindowsRetained, pane_geoms,
};
use crate::keybindings::{KeyBinding, ModeKeys, parse_list_keys, parse_prefix};
use crate::output::PaneOutput;
use crate::state::{ConnectionState, next_state};
use bevy::prelude::Commands;
use crossbeam_channel::Receiver;
use std::collections::{HashMap, HashSet};
use tmux_control::{ClientEvent, CommandId, ControlEvent, TransportEvent};
use tmux_control_parser::{PaneId, WindowId};

/// Upper bound on events drained per frame, so a pane flooding `%output`
/// cannot stall the schedule with unbounded parse/apply work in one tick;
/// any remainder stays queued and is drained on the next frame.
const MAX_EVENTS_PER_FRAME: usize = 4096;

/// Drains up to [`MAX_EVENTS_PER_FRAME`] currently-available transport events
/// from `events`, logging each. Non-blocking: returns once the channel is
/// empty or the per-frame cap is hit.
pub(crate) fn drain_transport(events: &Receiver<TransportEvent>) -> Vec<TransportEvent> {
    let mut drained = Vec::new();
    while drained.len() < MAX_EVENTS_PER_FRAME {
        match events.try_recv() {
            Ok(event) => {
                log_transport_event(&event);
                drained.push(event);
            }
            Err(_) => break,
        }
    }
    drained
}

/// Folds `events` through [`next_state`] from `current`, returning the resulting
/// `ConnectionState` if the batch changed it, or `None` if it ended unchanged.
///
/// Returning the next state (rather than mutating in place) lets the caller
/// write it back through `ResMut` only on a real transition, so change
/// detection fires once per transition instead of every frame.
pub(crate) fn advance_state(
    current: &ConnectionState,
    events: &[TransportEvent],
) -> Option<ConnectionState> {
    let mut next: Option<ConnectionState> = None;
    for event in events {
        next = Some(next_state(next.as_ref().unwrap_or(current), event));
    }
    next.filter(|n| n != current)
}

/// Translates a drained transport batch into global projection events, in
/// stream order, triggering each via `commands`. The enumeration reply (the
/// `CommandComplete` whose id matches `pending`) is decomposed into per-row
/// `TmuxWindowAdded` + `TmuxLayoutChanged` (+ `TmuxActiveWindowChanged` for the
/// active row), followed by one `TmuxWindowsRetained` prune. Untracked events
/// (e.g. `%output`) are ignored here (routed separately as `PaneOutput`).
pub(crate) fn trigger_events(
    commands: &mut Commands,
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) {
    for event in events {
        match event {
            TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
                trigger_notification(commands, notification);
            }
            TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. })
                if *pending == Some(*id) =>
            {
                *pending = None;
                if *ok {
                    trigger_seed(commands, output);
                } else {
                    tracing::warn!("list-windows enumeration command failed");
                }
            }
            _ => {}
        }
    }
}

/// Returns the client name from a `CommandComplete` whose id matches
/// `pending` (first non-empty trimmed output line), and clears `pending`.
///
/// Iterates `events` and looks for `CommandComplete { ok: true, .. }` whose
/// id equals `*pending`. On a match the first non-empty trimmed output line is
/// returned and `*pending` is set to `None`. Returns `None` when no matching
/// event exists in the batch or the output is blank.
pub(crate) fn take_client_name(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<String> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return output
                    .iter()
                    .map(|line| line.trim())
                    .find(|line| !line.is_empty())
                    .map(str::to_owned);
            } else {
                tracing::warn!("client-name query command failed");
                return None;
            }
        }
    }
    None
}

/// Drains matching `capture-pane` replies from `events`, returning a
/// [`PaneOutput`] seeding each captured pane's initial screen.
///
/// For every `CommandComplete` whose id is in `capture_pending`, the entry is
/// removed and (on success) its body lines are joined with CRLF into VT bytes
/// fed to the pane like ordinary `%output`. tmux `-CC` does not replay existing
/// content on attach, so this seeds the first paint; the live `%output` stream
/// keeps it current thereafter.
pub(crate) fn take_pane_captures(
    capture_pending: &mut HashMap<CommandId, PaneId>,
    events: &[TransportEvent],
) -> Vec<PaneOutput> {
    let mut out = Vec::new();
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && let Some(pane) = capture_pending.remove(id)
        {
            if *ok {
                out.push(PaneOutput {
                    pane,
                    data: capture_to_bytes(output),
                });
            } else {
                tracing::warn!(pane = pane.0, "capture-pane command failed");
            }
        }
    }
    out
}

/// Joins `capture-pane -p -e` reply lines into VT bytes for seeding a pane's
/// screen: a cursor-home + clear-screen prefix so the snapshot repaints from a
/// clean grid (the reply can arrive after live `%output` has already moved the
/// cursor — without the reset the rows would stack and duplicate), then the rows
/// CRLF-joined (the reply omits line terminators).
fn capture_to_bytes(lines: &[String]) -> Vec<u8> {
    let mut bytes = b"\x1b[H\x1b[2J".to_vec();
    bytes.extend_from_slice(lines.join("\r\n").as_bytes());
    bytes
}

/// Returns the active `(window, pane)` from a `CommandComplete` whose id matches
/// `pending` (parsing the `@N %M` reply line), clearing `pending`.
///
/// Used to seed the `ActivePane` marker on attach, since tmux does not emit
/// `%window-pane-changed` then. Returns `None` when no matching reply is in the
/// batch or the line does not parse.
pub(crate) fn take_active_pane(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<(WindowId, PaneId)> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return output.iter().find_map(|line| parse_active_pane(line));
            }
            tracing::warn!("active-pane query command failed");
            return None;
        }
    }
    None
}

/// Returns parsed key bindings from a `CommandComplete` matching `pending`
/// (running `parse_list_keys` on the reply), clearing `pending`. Returns `None`
/// when no matching reply is in the batch.
pub(crate) fn take_keybindings(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<Vec<KeyBinding>> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(parse_list_keys(output));
            }
            tracing::warn!("list-keys command failed");
            return None;
        }
    }
    None
}

/// Returns the prefix-key set from a `CommandComplete` matching `pending`
/// (running `parse_prefix` on the first reply line), clearing `pending`.
pub(crate) fn take_prefix_keys(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<HashSet<String>> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(
                    output
                        .first()
                        .map(|line| parse_prefix(line))
                        .unwrap_or_default(),
                );
            }
            tracing::warn!("prefix query command failed");
            return None;
        }
    }
    None
}

/// Returns the `ModeKeys` from a `CommandComplete` matching `pending`
/// (parsing `#{mode-keys}`), clearing `pending`.
pub(crate) fn take_mode_keys(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<ModeKeys> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(
                    output
                        .first()
                        .map(|l| ModeKeys::parse(l))
                        .unwrap_or_default(),
                );
            }
            tracing::warn!("mode-keys query failed");
            return None;
        }
    }
    None
}

/// Parses an `@N %M` line into `(WindowId, PaneId)`.
fn parse_active_pane(line: &str) -> Option<(WindowId, PaneId)> {
    let mut parts = line.split_whitespace();
    let window = parts.next()?.strip_prefix('@')?.parse().ok()?;
    let pane = parts.next()?.strip_prefix('%')?.parse().ok()?;
    Some((WindowId(window), PaneId(pane)))
}

fn trigger_notification(commands: &mut Commands, event: &ControlEvent) {
    match event {
        ControlEvent::SessionChanged { session, name } => {
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
        ControlEvent::WindowClose { window } => {
            commands.trigger(TmuxWindowClosed { window: *window });
        }
        ControlEvent::WindowRenamed { window, name } => {
            commands.trigger(TmuxWindowRenamed {
                window: *window,
                name: name.clone(),
            });
        }
        ControlEvent::LayoutChange { window, layout, .. } => {
            commands.trigger(TmuxLayoutChanged {
                window: *window,
                panes: pane_geoms(layout),
            });
        }
        ControlEvent::WindowPaneChanged { window, pane } => {
            commands.trigger(TmuxActivePaneChanged {
                window: *window,
                pane: *pane,
            });
        }
        _ => {}
    }
}

fn trigger_seed(commands: &mut Commands, output: &[String]) {
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
        commands.trigger(TmuxLayoutChanged {
            window: row.id,
            panes: pane_geoms(&row.layout),
        });
        if row.active {
            commands.trigger(TmuxActiveWindowChanged { window: row.id });
        }
        ids.push(row.id);
    }
    commands.trigger(TmuxWindowsRetained { windows: ids });
}

/// Emits a `tracing` line describing a single transport event.
fn log_transport_event(event: &TransportEvent) {
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
    use crate::enumerate::EnumerationState;
    use bevy::prelude::*;
    use crossbeam_channel::unbounded;
    use tmux_control::{CommandId, ControlEvent};
    use tmux_control_parser::WindowId;

    fn window_add(id: u32) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(id),
        }))
    }

    #[test]
    fn drain_then_advance_state_attaches() {
        let (tx, rx) = unbounded();
        tx.send(window_add(1)).unwrap();
        let drained = drain_transport(&rx);
        let next = advance_state(&ConnectionState::Connecting, &drained);
        assert_eq!(next, Some(ConnectionState::Attached));
    }

    #[test]
    fn take_client_name_extracts_from_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(7),
            number: 0,
            ok: true,
            output: vec!["main-client".to_string()],
        })];
        let mut pending = Some(CommandId(7));
        assert_eq!(
            take_client_name(&mut pending, &events),
            Some("main-client".to_string())
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn take_pane_captures_seeds_matching_reply_as_output() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(5),
            number: 0,
            ok: true,
            output: vec!["line one".to_string(), "line two".to_string()],
        })];
        let mut capture_pending = HashMap::from([(CommandId(5), PaneId(88))]);
        let out = take_pane_captures(&mut capture_pending, &events);
        assert_eq!(
            out,
            vec![PaneOutput {
                pane: PaneId(88),
                data: b"\x1b[H\x1b[2Jline one\r\nline two".to_vec(),
            }]
        );
        assert!(capture_pending.is_empty());
    }

    #[test]
    fn take_active_pane_parses_window_and_pane() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(9),
            number: 0,
            ok: true,
            output: vec!["@7 %88".to_string()],
        })];
        let mut pending = Some(CommandId(9));
        assert_eq!(
            take_active_pane(&mut pending, &events),
            Some((WindowId(7), PaneId(88)))
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn take_pane_captures_drops_failed_reply_without_output() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(5),
            number: 0,
            ok: false,
            output: vec![],
        })];
        let mut capture_pending = HashMap::from([(CommandId(5), PaneId(88))]);
        let out = take_pane_captures(&mut capture_pending, &events);
        assert!(out.is_empty());
        assert!(
            capture_pending.is_empty(),
            "failed capture is still cleared"
        );
    }

    #[test]
    fn take_client_name_unmatched_id_returns_none_and_keeps_pending() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(8),
            number: 0,
            ok: true,
            output: vec!["main-client".to_string()],
        })];
        let mut pending = Some(CommandId(7));
        assert_eq!(take_client_name(&mut pending, &events), None);
        assert_eq!(pending, Some(CommandId(7)));
    }

    #[test]
    fn take_client_name_trims_whitespace_from_output() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(3),
            number: 0,
            ok: true,
            output: vec!["  /dev/ttys001  ".to_string()],
        })];
        let mut pending = Some(CommandId(3));
        assert_eq!(
            take_client_name(&mut pending, &events),
            Some("/dev/ttys001".to_string())
        );
    }

    #[test]
    fn seed_reply_triggers_per_row_events_then_retain() {
        use crate::events::{TmuxLayoutChanged, TmuxWindowAdded, TmuxWindowsRetained};
        use std::sync::{Arc, Mutex};

        #[derive(Resource, Default, Clone)]
        struct Log(Arc<Mutex<Vec<String>>>);

        #[derive(Resource)]
        struct Batch(Vec<TransportEvent>);

        fn run(
            mut commands: Commands,
            mut enumeration: ResMut<EnumerationState>,
            batch: Res<Batch>,
        ) {
            trigger_events(&mut commands, &mut enumeration.pending, &batch.0);
        }

        let mut app = App::new();
        app.init_resource::<Log>();
        app.init_resource::<EnumerationState>();
        app.world_mut().resource_mut::<EnumerationState>().pending = Some(CommandId(1));
        app.insert_resource(Batch(vec![TransportEvent::Protocol(
            ClientEvent::CommandComplete {
                id: CommandId(1),
                number: 0,
                ok: true,
                output: vec!["1\t@1\t0\tabcd,80x24,0,0,5\tx\tmain".to_string()],
            },
        )]));
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
        app.add_systems(Update, run);

        let log = app.world().resource::<Log>().clone();
        app.update();

        assert_eq!(*log.0.lock().unwrap(), vec!["add@1", "layout@1", "retain1"]);
        assert_eq!(app.world().resource::<EnumerationState>().pending, None);
    }

    #[test]
    fn take_keybindings_parses_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(11),
            number: 0,
            ok: true,
            output: vec!["bind-key -T root M-i split-window -h".to_string()],
        })];
        let mut pending = Some(CommandId(11));
        let got = take_keybindings(&mut pending, &events).expect("a parsed reply");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "M-i");
        assert_eq!(got[0].command, "split-window -h");
        assert_eq!(pending, None);
    }

    #[test]
    fn take_keybindings_unmatched_keeps_pending() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(2),
            number: 0,
            ok: true,
            output: vec![],
        })];
        let mut pending = Some(CommandId(11));
        assert!(take_keybindings(&mut pending, &events).is_none());
        assert_eq!(pending, Some(CommandId(11)));
    }

    #[test]
    fn take_prefix_keys_parses_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(12),
            number: 0,
            ok: true,
            output: vec!["C-b None".to_string()],
        })];
        let mut pending = Some(CommandId(12));
        let got = take_prefix_keys(&mut pending, &events).expect("a parsed reply");
        assert_eq!(got, std::collections::HashSet::from(["C-b".to_string()]));
        assert_eq!(pending, None);
    }

    #[test]
    fn take_mode_keys_parses_vi() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(21),
            number: 0,
            ok: true,
            output: vec!["vi".to_string()],
        })];
        let mut pending = Some(CommandId(21));
        assert_eq!(take_mode_keys(&mut pending, &events), Some(ModeKeys::Vi));
        assert_eq!(pending, None);
    }

    #[test]
    fn take_mode_keys_defaults_emacs_on_other() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(22),
            number: 0,
            ok: true,
            output: vec!["emacs".to_string()],
        })];
        let mut pending = Some(CommandId(22));
        assert_eq!(take_mode_keys(&mut pending, &events), Some(ModeKeys::Emacs));
    }
}
