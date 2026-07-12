//! Host keyboard input primitives: registers the `KeyboardInput` message stream
//! and provides the key/modifier mapping helpers used elsewhere in the input
//! pipeline — `bevy_key_to_terminal_key` (the Default applier's raw-key
//! forwarding) and `current_terminal_modifiers` (the Default applier plus the
//! mouse dispatch).

use crate::input::current_modifiers;
use crate::input::keyboard::handler::KeyboardHandlerPlugin;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use orzma_tty_engine::{TerminalKey, TerminalModifiers};

mod handler;
pub mod key_effect;

/// Registers the `KeyboardInput` message stream.
pub(super) struct KeyboardInputPlugin;

impl Plugin for KeyboardInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(KeyboardHandlerPlugin)
            .add_message::<KeyboardInput>();
    }
}

/// Returns the terminal modifier state from the `ButtonInput<KeyCode>` resource.
pub(crate) fn current_terminal_modifiers(keys: &ButtonInput<KeyCode>) -> TerminalModifiers {
    let m = current_modifiers(keys);
    TerminalModifiers {
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Maps a Bevy logical `Key` to the engine's `TerminalKey`, or `None` for keys
/// with no terminal representation (bare modifiers, function keys, etc.).
pub(crate) fn bevy_key_to_terminal_key(logical_key: &Key) -> Option<TerminalKey> {
    match logical_key {
        Key::Character(s) => Some(TerminalKey::Text(s.to_string())),
        Key::Space => Some(TerminalKey::Text(" ".to_string())),
        Key::Enter => Some(TerminalKey::Enter),
        Key::Backspace => Some(TerminalKey::Backspace),
        Key::Tab => Some(TerminalKey::Tab),
        Key::Escape => Some(TerminalKey::Escape),
        Key::Delete => Some(TerminalKey::Delete),
        Key::ArrowUp => Some(TerminalKey::ArrowUp),
        Key::ArrowDown => Some(TerminalKey::ArrowDown),
        Key::ArrowLeft => Some(TerminalKey::ArrowLeft),
        Key::ArrowRight => Some(TerminalKey::ArrowRight),
        Key::Home => Some(TerminalKey::Home),
        Key::End => Some(TerminalKey::End),
        Key::PageUp => Some(TerminalKey::PageUp),
        Key::PageDown => Some(TerminalKey::PageDown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_char_maps_to_text() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("a".into())),
            Some(TerminalKey::Text("a".to_string()))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("あ".into())),
            Some(TerminalKey::Text("あ".to_string()))
        );
    }

    #[test]
    fn space_maps_to_text() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Space),
            Some(TerminalKey::Text(" ".to_string()))
        );
    }

    #[test]
    fn control_keys_map_correctly() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Enter),
            Some(TerminalKey::Enter)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Backspace),
            Some(TerminalKey::Backspace)
        );
        assert_eq!(bevy_key_to_terminal_key(&Key::Tab), Some(TerminalKey::Tab));
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Escape),
            Some(TerminalKey::Escape)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Delete),
            Some(TerminalKey::Delete)
        );
    }

    #[test]
    fn navigation_keys_map_correctly() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowUp),
            Some(TerminalKey::ArrowUp)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowDown),
            Some(TerminalKey::ArrowDown)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowLeft),
            Some(TerminalKey::ArrowLeft)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowRight),
            Some(TerminalKey::ArrowRight)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Home),
            Some(TerminalKey::Home)
        );
        assert_eq!(bevy_key_to_terminal_key(&Key::End), Some(TerminalKey::End));
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageUp),
            Some(TerminalKey::PageUp)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageDown),
            Some(TerminalKey::PageDown)
        );
    }

    #[test]
    fn modifier_and_unrecognized_keys_return_none() {
        assert_eq!(bevy_key_to_terminal_key(&Key::Shift), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Control), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Alt), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Super), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::F1), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Insert), None);
    }
}
