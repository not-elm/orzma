//! `RequestTermKeyInput` and the observer that forwards it to the
//! target terminal's PTY.

use crate::OrzmaTermHandle;
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

/// Registers the [`RequestTermKeyInput`] apply observer.
pub(super) struct KeyInputPlugin;

impl Plugin for KeyInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_request_term_key_input);
    }
}

fn apply_request_term_key_input(
    e: On<RequestTermKeyInput>,
    mut terms: Query<&mut OrzmaTermHandle>,
) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.write_key_input(&e.key, &e.modifiers)
    {
        error!(%err);
    }
}
