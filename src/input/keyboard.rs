//! Host keyboard input primitives: maps host key/modifier state to the
//! terminal's encoding.

use crate::input::current_modifiers;
use crate::input::keyboard::handler::KeyboardHandlerPlugin;
use crate::input::keyboard::held_modifiers::HeldModifiersPlugin;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use orzma_configs::shortcuts::Modifiers;
use orzma_tty::prelude::{
    CursorKey, EditingKey, FunctionKey, KeyText, TerminalKey, TerminalModifiers,
};

mod handler;
mod held_modifiers;
pub mod key_effect;

/// Adds the `KeyboardInput` message stream.
pub(super) struct KeyboardInputPlugin;

impl Plugin for KeyboardInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((KeyboardHandlerPlugin, HeldModifiersPlugin))
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
/// keys with no terminal representation (bare modifiers, function keys above
/// F12, or character text that encodes to an empty string).
pub(crate) fn bevy_key_to_terminal_key(logical_key: &Key) -> Option<TerminalKey> {
    match logical_key {
        Key::Character(s) => KeyText::new(s.to_string()).map(TerminalKey::Character),
        Key::Space => KeyText::new(" ").map(TerminalKey::Character),
        Key::Enter => Some(TerminalKey::Enter),
        Key::Backspace => Some(TerminalKey::Backspace),
        Key::Tab => Some(TerminalKey::Tab),
        Key::Escape => Some(TerminalKey::Escape),
        Key::Insert => Some(TerminalKey::Editing(EditingKey::Insert)),
        Key::Delete => Some(TerminalKey::Editing(EditingKey::Delete)),
        Key::ArrowUp => Some(TerminalKey::Cursor(CursorKey::Up)),
        Key::ArrowDown => Some(TerminalKey::Cursor(CursorKey::Down)),
        Key::ArrowLeft => Some(TerminalKey::Cursor(CursorKey::Left)),
        Key::ArrowRight => Some(TerminalKey::Cursor(CursorKey::Right)),
        Key::Home => Some(TerminalKey::Cursor(CursorKey::Home)),
        Key::End => Some(TerminalKey::Cursor(CursorKey::End)),
        Key::PageUp => Some(TerminalKey::Editing(EditingKey::PageUp)),
        Key::PageDown => Some(TerminalKey::Editing(EditingKey::PageDown)),
        Key::F1 => Some(TerminalKey::Function(FunctionKey::F1)),
        Key::F2 => Some(TerminalKey::Function(FunctionKey::F2)),
        Key::F3 => Some(TerminalKey::Function(FunctionKey::F3)),
        Key::F4 => Some(TerminalKey::Function(FunctionKey::F4)),
        Key::F5 => Some(TerminalKey::Function(FunctionKey::F5)),
        Key::F6 => Some(TerminalKey::Function(FunctionKey::F6)),
        Key::F7 => Some(TerminalKey::Function(FunctionKey::F7)),
        Key::F8 => Some(TerminalKey::Function(FunctionKey::F8)),
        Key::F9 => Some(TerminalKey::Function(FunctionKey::F9)),
        Key::F10 => Some(TerminalKey::Function(FunctionKey::F10)),
        Key::F11 => Some(TerminalKey::Function(FunctionKey::F11)),
        Key::F12 => Some(TerminalKey::Function(FunctionKey::F12)),
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
            Some(TerminalKey::Editing(EditingKey::Delete))
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
            Some(TerminalKey::Cursor(CursorKey::Up))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowDown),
            Some(TerminalKey::Cursor(CursorKey::Down))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowLeft),
            Some(TerminalKey::Cursor(CursorKey::Left))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowRight),
            Some(TerminalKey::Cursor(CursorKey::Right))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Home),
            Some(TerminalKey::Cursor(CursorKey::Home))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::End),
            Some(TerminalKey::Cursor(CursorKey::End))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageUp),
            Some(TerminalKey::Editing(EditingKey::PageUp))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageDown),
            Some(TerminalKey::Editing(EditingKey::PageDown))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Insert),
            Some(TerminalKey::Editing(EditingKey::Insert))
        );
    }

    /// Asserts that F1 through F12 map to their `FunctionKey` variants.
    ///
    /// Case: the user presses F1 for help or F10 to quit in htop or Midnight
    /// Commander.
    #[test]
    fn function_keys_map_to_their_function_key_variants() {
        let cases = [
            (Key::F1, FunctionKey::F1),
            (Key::F2, FunctionKey::F2),
            (Key::F3, FunctionKey::F3),
            (Key::F4, FunctionKey::F4),
            (Key::F5, FunctionKey::F5),
            (Key::F6, FunctionKey::F6),
            (Key::F7, FunctionKey::F7),
            (Key::F8, FunctionKey::F8),
            (Key::F9, FunctionKey::F9),
            (Key::F10, FunctionKey::F10),
            (Key::F11, FunctionKey::F11),
            (Key::F12, FunctionKey::F12),
        ];
        for (key, function) in cases {
            assert_eq!(
                bevy_key_to_terminal_key(&key),
                Some(TerminalKey::Function(function)),
                "{key:?}"
            );
        }
    }

    /// Asserts bare modifier keys and unmapped keys (e.g. F13 and above)
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
        assert_eq!(bevy_key_to_terminal_key(&Key::F13), None);
    }

    /// Asserts an empty character payload maps to `None` rather than a
    /// zero-length `TerminalKey::Character`, since `KeyText` cannot represent
    /// empty text.
    ///
    /// Case: a platform IME or compose sequence delivers a `Key::Character`
    /// event carrying an empty string.
    #[test]
    fn empty_character_text_maps_to_none() {
        assert_eq!(bevy_key_to_terminal_key(&Key::Character("".into())), None);
    }
}
