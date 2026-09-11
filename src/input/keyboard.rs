//! Host keyboard input primitives: registers the `KeyboardInput` message stream
//! and provides the key/modifier mapping helpers used elsewhere in the input
//! pipeline — `bevy_key_to_terminal_key` (the Default applier's raw-key
//! forwarding) and `current_terminal_modifiers` (the Default applier plus the
//! mouse dispatch).

use crate::input::current_modifiers;
use crate::input::keyboard::handler::KeyboardHandlerPlugin;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use orzma_configs::shortcuts::Modifiers;
use orzma_tty::prelude::{KeyText, TerminalKey, TerminalModifiers};

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
    terminal_modifiers(current_modifiers(keys))
}

/// The terminal encoder's view of a host modifier state.
pub(crate) fn terminal_modifiers(m: Modifiers) -> TerminalModifiers {
    TerminalModifiers {
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Maps a Bevy logical `Key` to `orzma_tty`'s `TerminalKey`, or `None` for
/// keys with no terminal representation (bare modifiers, function keys, or
/// character text that encodes to an empty string).
pub(crate) fn bevy_key_to_terminal_key(logical_key: &Key) -> Option<TerminalKey> {
    match logical_key {
        Key::Character(s) => KeyText::new(s.to_string()).map(TerminalKey::Character),
        Key::Space => KeyText::new(" ").map(TerminalKey::Character),
        Key::Enter => Some(TerminalKey::Enter),
        Key::Backspace => Some(TerminalKey::Backspace),
        Key::Tab => Some(TerminalKey::Tab),
        Key::Escape => Some(TerminalKey::Escape),
        Key::Insert => Some(TerminalKey::Insert),
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

    /// Asserts printable characters map to `TerminalKey::Character`, wrapping a
    /// non-empty `KeyText` — ASCII and multibyte alike.
    ///
    /// Case: the user types `a` or an IME-composed `あ` and the key handler
    /// forwards it to the terminal.
    #[test]
    fn printable_char_maps_to_character() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("a".into())),
            Some(TerminalKey::Character(KeyText::new("a").unwrap()))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("あ".into())),
            Some(TerminalKey::Character(KeyText::new("あ").unwrap()))
        );
    }

    /// Asserts the space bar maps to `TerminalKey::Character(" ")`.
    ///
    /// Case: the user presses Space in the terminal.
    #[test]
    fn space_maps_to_character() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Space),
            Some(TerminalKey::Character(KeyText::new(" ").unwrap()))
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

    /// Asserts that the navigation and editing keys map to their
    /// `TerminalKey` variants.
    ///
    /// Case: the user moves through a document with the arrow keys, Home,
    /// End, PageUp, and PageDown, and toggles overwrite with Insert.
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
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Insert),
            Some(TerminalKey::Insert)
        );
    }

    /// Asserts bare modifier keys and unmapped keys (e.g. function keys)
    /// return `None`.
    ///
    /// Case: the user presses a lone Shift/Ctrl/Alt/Super, or a key this codec
    /// does not wire, and the dispatcher must send nothing to the PTY.
    #[test]
    fn modifier_and_unrecognized_keys_return_none() {
        assert_eq!(bevy_key_to_terminal_key(&Key::Shift), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Control), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Alt), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Super), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::F1), None);
    }

    /// Asserts an empty character payload maps to `None` rather than a
    /// zero-length `TerminalKey::Character`, since `KeyText` cannot represent
    /// empty text (D12 of the engine-swap design).
    ///
    /// Case: a platform IME or compose sequence delivers a `Key::Character`
    /// event carrying an empty string.
    #[test]
    fn empty_character_text_maps_to_none() {
        assert_eq!(bevy_key_to_terminal_key(&Key::Character("".into())), None);
    }
}
