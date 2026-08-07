//! Inbound request `EntityEvent`s: commands the host UI fires AT a
//! terminal entity, as opposed to the outbound `Term*Signal`s in
//! `signals.rs` that are drained FROM the VT.

use bevy::prelude::*;
use orzma_term::prelude::{TerminalKey, TerminalModifiers};

/// Fired by the host UI to forward a key press to a specific terminal entity.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermKeyInput {
    #[event_target]
    pub terminal: Entity,
    /// The logical key pressed (character or named key, pre-encoding).
    pub key: TerminalKey,
    /// Modifier state at press time; feeds the encoder, not a raw HID state.
    pub modifiers: TerminalModifiers,
}
