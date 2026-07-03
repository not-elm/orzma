//! Backend-bytes event for PTY-less terminal surfaces; the host owns the
//! observer that routes it to the real backend (`crate::input::tmux::forward`).

use bevy::prelude::*;

/// Terminal input bytes destined for the backend of `entity` (a PTY for a
/// local terminal, or tmux `send-keys` for a control-mode pane). Emitted by the
/// mouse apply observer when the terminal has no `PtyHandle`; the host owns the
/// observer that routes it to the real backend.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalForwardInput {
    /// The terminal entity whose backend should receive `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The raw bytes to deliver to the backend.
    pub bytes: Vec<u8>,
}
