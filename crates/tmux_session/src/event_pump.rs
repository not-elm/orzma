//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the `ProjectionModel`.

use crate::model::ProjectionModel;
use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, TransportEvent};

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

/// Routes notification events into the projection model, returning `true`
/// if any event mutated tracked state (so callers can skip change
/// propagation when only untracked events — e.g. pane output — arrived).
pub(crate) fn route_to_model(model: &mut ProjectionModel, events: &[TransportEvent]) -> bool {
    let mut changed = false;
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::Notification(notification)) = event {
            changed |= model.apply_event(notification);
        }
    }
    changed
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
    use tmux_control::ControlEvent;
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
    fn route_to_model_applies_notifications() {
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
        assert!(route_to_model(&mut model, &drained));
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.windows[0].panes.len(), 1);
        assert_eq!(model.windows[0].panes[0].id, PaneId(4));
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
        assert!(!route_to_model(&mut model, &drained));
        assert!(model.windows.is_empty());
    }
}
