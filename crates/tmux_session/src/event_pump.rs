//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the global projection events the observers consume.

use crate::enumerate::parse_window_rows;
use crate::events::{
    TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxLayoutChanged, TmuxSessionChanged,
    TmuxWindowAdded, TmuxWindowClosed, TmuxWindowRenamed, TmuxWindowsRetained, pane_geoms,
};
use crate::state::{ConnectionState, next_state};
use bevy::prelude::Commands;
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, CommandId, ControlEvent, TransportEvent};

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

/// Advances `state` through [`next_state`] for each event, returning `true`
/// if the state actually changed.
pub(crate) fn advance_state(state: &mut ConnectionState, events: &[TransportEvent]) -> bool {
    let mut changed = false;
    for event in events {
        let next = next_state(state, event);
        if *state != next {
            *state = next;
            changed = true;
        }
    }
    changed
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
        let mut state = ConnectionState::Connecting;
        advance_state(&mut state, &drained);
        assert_eq!(state, ConnectionState::Attached);
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

        fn run(mut commands: Commands, mut enumeration: ResMut<EnumerationState>, batch: Res<Batch>) {
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
            log.0.lock().unwrap().push(format!("layout@{}", ev.window.0));
        });
        app.add_observer(|ev: On<TmuxWindowsRetained>, log: Res<Log>| {
            log.0.lock().unwrap().push(format!("retain{}", ev.windows.len()));
        });
        app.add_systems(Update, run);

        let log = app.world().resource::<Log>().clone();
        app.update();

        assert_eq!(*log.0.lock().unwrap(), vec!["add@1", "layout@1", "retain1"]);
        assert_eq!(app.world().resource::<EnumerationState>().pending, None);
    }
}
