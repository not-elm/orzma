//! Draining and logging of tmux transport events.

use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, TransportEvent};

/// Drains every currently-available transport event from `events`, logs
/// each one, and advances `state` through [`next_state`]. Non-blocking:
/// returns once the channel is empty for now.
pub(crate) fn drain_events(state: &mut ConnectionState, events: &Receiver<TransportEvent>) {
    while let Ok(event) = events.try_recv() {
        log_transport_event(&event);
        let next = next_state(state, &event);
        if *state != next {
            *state = next;
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
    use tmux_control_parser::WindowId;

    fn notification() -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(1),
        }))
    }

    #[test]
    fn drains_until_empty_and_attaches() {
        let (tx, rx) = unbounded();
        tx.send(notification()).unwrap();
        tx.send(notification()).unwrap();
        let mut state = ConnectionState::Connecting;
        drain_events(&mut state, &rx);
        assert_eq!(state, ConnectionState::Attached);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn close_after_attach_transitions_to_detached() {
        let (tx, rx) = unbounded();
        tx.send(notification()).unwrap();
        tx.send(TransportEvent::Closed {
            reason: "eof".to_string(),
        })
        .unwrap();
        let mut state = ConnectionState::Connecting;
        drain_events(&mut state, &rx);
        assert_eq!(state, ConnectionState::Detached);
    }

    #[test]
    fn empty_channel_leaves_state_untouched() {
        let (_tx, rx) = unbounded::<TransportEvent>();
        let mut state = ConnectionState::Idle;
        drain_events(&mut state, &rx);
        assert_eq!(state, ConnectionState::Idle);
    }
}
