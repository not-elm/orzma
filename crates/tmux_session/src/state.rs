//! Connection lifecycle state and its transition rules.

use bevy::prelude::Resource;
use tmux_control::TransportEvent;

/// The tmux connection lifecycle, surfaced to the rest of the app.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// No connection attempt has been made yet.
    #[default]
    Idle,
    /// A `tmux -CC` process has been spawned but no event has arrived yet.
    Connecting,
    /// The transport is live (at least one event has been received).
    Attached,
    /// The transport closed after having been attached.
    Detached,
    /// The transport closed before attaching, or closed abnormally.
    Error {
        /// Human-readable close reason.
        reason: String,
    },
}

/// Returns the next [`ConnectionState`] for `current` given `event`.
///
/// Any protocol event proves the transport is live, so it moves to
/// `Attached`. A close moves to `Detached` if previously attached, or to
/// `Error` otherwise (e.g. a close during `Connecting`).
pub(crate) fn next_state(current: &ConnectionState, event: &TransportEvent) -> ConnectionState {
    match event {
        TransportEvent::Protocol(_) => ConnectionState::Attached,
        TransportEvent::Closed { reason } => match current {
            ConnectionState::Attached => ConnectionState::Detached,
            _ => ConnectionState::Error {
                reason: reason.clone(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control::{ClientEvent, ControlEvent};
    use tmux_control_parser::WindowId;

    fn notification() -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(1),
        }))
    }

    #[test]
    fn protocol_event_attaches_from_connecting() {
        assert_eq!(
            next_state(&ConnectionState::Connecting, &notification()),
            ConnectionState::Attached
        );
    }

    #[test]
    fn protocol_event_keeps_attached() {
        assert_eq!(
            next_state(&ConnectionState::Attached, &notification()),
            ConnectionState::Attached
        );
    }

    #[test]
    fn close_after_attached_is_detached() {
        let close = TransportEvent::Closed {
            reason: "eof".to_string(),
        };
        assert_eq!(
            next_state(&ConnectionState::Attached, &close),
            ConnectionState::Detached
        );
    }

    #[test]
    fn close_while_connecting_is_error() {
        let close = TransportEvent::Closed {
            reason: "boom".to_string(),
        };
        assert_eq!(
            next_state(&ConnectionState::Connecting, &close),
            ConnectionState::Error {
                reason: "boom".to_string()
            }
        );
    }
}
