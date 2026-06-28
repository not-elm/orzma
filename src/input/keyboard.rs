//! Host keyboard primitives shared by the terminal keyboard dispatch and the
//! mouse dispatch (modifier reading). Gains the relocated `dispatch_input` in Task 5.

#![expect(
    dead_code,
    reason = "primitives wired into the host mouse/keyboard dispatch in Task 4"
)]

use bevy::prelude::*;
use ozma_tty_engine::TerminalModifiers;

/// Returns the terminal modifier state from the `ButtonInput<KeyCode>` resource.
pub(crate) fn current_terminal_modifiers(keys: &ButtonInput<KeyCode>) -> TerminalModifiers {
    TerminalModifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}
