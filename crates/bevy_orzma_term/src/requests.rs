//! Inbound request `EntityEvent`s: commands the host UI fires AT a
//! terminal entity, as opposed to the outbound `Term*Signal`s in
//! `signals.rs` that are drained FROM the VT.

use bevy::prelude::*;
use orzma_term::prelude::{TerminalKey, TerminalModifiers};

use crate::OrzmaTermHandle;

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

pub(crate) struct OrzmaEventRequestPlugin;

impl Plugin for OrzmaEventRequestPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_request_term_key_input);
    }
}

fn apply_request_term_key_input(
    e: On<RequestTermKeyInput>,
    mut terms: Query<&mut OrzmaTermHandle>,
) {
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        if let Err(e) = tty.write_key_input(&e.key, &e.modifiers) {
            error!(%e);
        }
    }
}
