//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the `ProjectionModel`.

use crate::model::ProjectionModel;
use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, TransportEvent};

/// Drains every currently-available transport event from `events`, logging
/// each. Non-blocking: returns once the channel is empty for now.
pub(crate) fn drain_transport(events: &Receiver<TransportEvent>) -> Vec<TransportEvent> {
    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        log_transport_event(&event);
        drained.push(event);
    }
    drained
}

/// Advances `state` through [`next_state`] for each drained event.
pub(crate) fn advance_state(state: &mut ConnectionState, events: &[TransportEvent]) {
    for event in events {
        let next = next_state(state, event);
        if *state != next {
            *state = next;
        }
    }
}

/// Routes notification events into the projection model.
pub(crate) fn route_to_model(model: &mut ProjectionModel, events: &[TransportEvent]) {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::Notification(notification)) = event {
            model.apply_event(notification);
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
        route_to_model(&mut model, &drained);
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.windows[0].panes.len(), 1);
        assert_eq!(model.windows[0].panes[0].id, PaneId(4));
    }
}
