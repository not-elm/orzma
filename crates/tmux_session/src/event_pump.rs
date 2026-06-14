//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the `ProjectionModel`.

use crate::enumerate::parse_window_rows;
use crate::model::ProjectionModel;
use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, CommandId, TransportEvent};

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

/// Applies a drained batch to the model in stream order: each notification
/// via [`ProjectionModel::apply_event`], and the enumeration reply (the
/// `CommandComplete` whose id matches `pending`) via [`seed_from_reply`].
///
/// Processing in order is load-bearing: a notification that follows the
/// `list-windows` reply in the stream is applied AFTER the seed, so the
/// wholesale window replacement in `seed_from_rows` cannot clobber fresher
/// same-batch state. Returns `true` if the model changed; clears `pending`
/// once the matching reply is consumed. Untracked events (e.g. pane output)
/// leave the model unchanged so callers can skip change propagation.
pub(crate) fn apply_events(
    model: &mut ProjectionModel,
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> bool {
    let mut changed = false;
    for event in events {
        match event {
            TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
                changed |= model.apply_event(notification);
            }
            TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. })
                if *pending == Some(*id) =>
            {
                *pending = None;
                if *ok {
                    changed |= seed_from_reply(model, output);
                } else {
                    tracing::warn!("list-windows enumeration command failed");
                }
            }
            _ => {}
        }
    }
    changed
}

/// Parses a `list-windows` reply and seeds it into `model`, returning `true`
/// on success. A malformed reply is logged and leaves the model untouched.
pub(crate) fn seed_from_reply(model: &mut ProjectionModel, output: &[String]) -> bool {
    match parse_window_rows(output) {
        Ok(rows) => {
            model.seed_from_rows(&rows);
            true
        }
        Err(error) => {
            tracing::warn!(error = %error, "failed to parse list-windows reply");
            false
        }
    }
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
    use crossbeam_channel::unbounded;
    use tmux_control::{CommandId, ControlEvent};
    use tmux_control_parser::{PaneId, WindowId, WindowLayout};

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
    fn apply_events_applies_notifications() {
        let (tx, rx) = unbounded();
        tx.send(window_add(1)).unwrap();
        tx.send(TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::LayoutChange {
                window: WindowId(1),
                layout: WindowLayout::parse(b"abcd,80x24,0,0,4").unwrap(),
                visible_layout: WindowLayout::parse(b"abcd,80x24,0,0,4").unwrap(),
                flags: String::new(),
            },
        )))
        .unwrap();
        let drained = drain_transport(&rx);
        let mut model = ProjectionModel::default();
        let mut pending = None;
        assert!(apply_events(&mut model, &mut pending, &drained));
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.windows[0].panes.len(), 1);
        assert_eq!(model.windows[0].panes[0].id, PaneId(4));
    }

    #[test]
    fn apply_events_seeds_reply_then_applies_later_notification() {
        // The reply seeds window @1; a WindowAdd @9 that follows it in the
        // stream must survive (the seed's window replacement must not clobber
        // a notification ordered after the reply).
        let reply = TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(1),
            number: 0,
            ok: true,
            output: vec!["1\t@1\tabcd,80x24,0,0,5\tx\tmain".to_string()],
        });
        let events = vec![reply, window_add(9)];
        let mut model = ProjectionModel::default();
        let mut pending = Some(CommandId(1));
        assert!(apply_events(&mut model, &mut pending, &events));
        assert_eq!(pending, None);
        let ids: Vec<_> = model.windows.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![WindowId(1), WindowId(9)]);
    }

    #[test]
    fn apply_events_ignores_unmatched_command_reply() {
        let reply = TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(2),
            number: 0,
            ok: true,
            output: vec!["1\t@1\tabcd,80x24,0,0,5\tx\tmain".to_string()],
        });
        let mut model = ProjectionModel::default();
        let mut pending = Some(CommandId(1));
        assert!(!apply_events(&mut model, &mut pending, &[reply]));
        assert_eq!(pending, Some(CommandId(1)));
        assert!(model.windows.is_empty());
    }

    #[test]
    fn seed_from_reply_populates_model() {
        let output = vec!["1\t@1\tabcd,80x24,0,0,5\tx\tmain".to_string()];
        let mut model = ProjectionModel::default();
        assert!(seed_from_reply(&mut model, &output));
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.windows[0].id, WindowId(1));
        assert_eq!(
            model.windows[0].panes.first().map(|p| p.id),
            Some(PaneId(5))
        );
    }

    #[test]
    fn seed_from_reply_rejects_malformed() {
        let output = vec!["garbage".to_string()];
        let mut model = ProjectionModel::default();
        assert!(!seed_from_reply(&mut model, &output));
        assert!(model.windows.is_empty());
    }

    #[test]
    fn output_only_batch_reports_no_model_change() {
        let (tx, rx) = unbounded();
        tx.send(TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::Output {
                pane: PaneId(1),
                data: vec![b'x'],
            },
        )))
        .unwrap();
        let drained = drain_transport(&rx);
        let mut model = ProjectionModel::default();
        let mut pending = None;
        assert!(!apply_events(&mut model, &mut pending, &drained));
        assert!(model.windows.is_empty());
    }
}
