//! Host keyboard input primitives: maps host key/modifier state to the
//! terminal's encoding.

use crate::input::current_modifiers;
use crate::input::keyboard::handler::KeyboardHandlerPlugin;
use crate::input::keyboard::held_modifiers::HeldModifiersPlugin;
use crate::input::keyboard::unshifted::UnshiftedKeysPlugin;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use orzma_configs::shortcuts::Modifiers;
use orzma_tty::prelude::{
    CursorKey, EditingKey, FunctionKey, KeyText, KeypadKey, TerminalKey, TerminalModifiers,
};

mod handler;
mod held_modifiers;
pub mod key_effect;
pub mod unshifted;

/// Adds the `KeyboardInput` message stream.
pub(super) struct KeyboardInputPlugin;

impl Plugin for KeyboardInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            KeyboardHandlerPlugin,
            HeldModifiersPlugin,
            UnshiftedKeysPlugin,
        ))
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

/// Maps a pressed key to `orzma_tty`'s `TerminalKey`, or `None` for keys with
/// no terminal representation (bare modifiers, function keys above F12,
/// character text that encodes to an empty string, or an unidentified key
/// without a held Ctrl, Alt, or Meta and a recorded un-shifted character).
///
/// `key_code` is the physical key; the numeric keypad's keys map to
/// `TerminalKey::Keypad` whatever the logical key says. `unshifted` is the
/// character the physical key types without Shift, and rides on a character
/// key's text.
pub(crate) fn bevy_key_to_terminal_key(
    logical_key: &Key,
    key_code: KeyCode,
    unshifted: Option<char>,
    mods: &TerminalModifiers,
) -> Option<TerminalKey> {
    if let Some(key) = keypad_key(logical_key, key_code) {
        return Some(key);
    }
    match logical_key {
        Key::Character(s) => KeyText::new(s.to_string()).map(|text| {
            TerminalKey::Character(match unshifted {
                Some(c) => text.with_unshifted(c),
                None => text,
            })
        }),
        Key::Unidentified(_) => unshifted
            .filter(|_| mods.ctrl || mods.alt || mods.meta)
            .map(|unshifted| TerminalKey::Unidentified { unshifted }),
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

fn keypad_key(logical_key: &Key, key_code: KeyCode) -> Option<TerminalKey> {
    let character_key = match key_code {
        KeyCode::Numpad0 => KeypadKey::Zero,
        KeyCode::Numpad1 => KeypadKey::One,
        KeyCode::Numpad2 => KeypadKey::Two,
        KeyCode::Numpad3 => KeypadKey::Three,
        KeyCode::Numpad4 => KeypadKey::Four,
        KeyCode::Numpad5 => KeypadKey::Five,
        KeyCode::Numpad6 => KeypadKey::Six,
        KeyCode::Numpad7 => KeypadKey::Seven,
        KeyCode::Numpad8 => KeypadKey::Eight,
        KeyCode::Numpad9 => KeypadKey::Nine,
        KeyCode::NumpadDecimal => KeypadKey::Decimal,
        KeyCode::NumpadComma => KeypadKey::Separator,
        KeyCode::NumpadDivide => KeypadKey::Divide,
        KeyCode::NumpadMultiply => KeypadKey::Multiply,
        KeyCode::NumpadSubtract => KeypadKey::Subtract,
        KeyCode::NumpadAdd => KeypadKey::Add,
        KeyCode::NumpadEqual => KeypadKey::Equal,
        KeyCode::NumpadEnter => {
            return matches!(logical_key, Key::Enter).then_some(TerminalKey::Keypad {
                key: KeypadKey::Enter,
                text: None,
            });
        }
        _ => return None,
    };
    let key = match logical_key {
        Key::Character(s) => {
            return Some(TerminalKey::Keypad {
                key: character_key,
                text: KeyText::new(s.to_string()),
            });
        }
        Key::ArrowUp => KeypadKey::Up,
        Key::ArrowDown => KeypadKey::Down,
        Key::ArrowLeft => KeypadKey::Left,
        Key::ArrowRight => KeypadKey::Right,
        Key::Home => KeypadKey::Home,
        Key::End => KeypadKey::End,
        Key::PageUp => KeypadKey::PageUp,
        Key::PageDown => KeypadKey::PageDown,
        Key::Insert => KeypadKey::Insert,
        Key::Delete => KeypadKey::Delete,
        _ if key_code == KeyCode::Numpad5 => KeypadKey::Begin,
        _ => return None,
    };
    Some(TerminalKey::Keypad { key, text: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::NativeKey;
    use orzma_tty::prelude::KeypadKey;

    fn map_key(key: &Key) -> Option<TerminalKey> {
        bevy_key_to_terminal_key(key, KeyCode::KeyA, None, &TerminalModifiers::default())
    }

    fn map_numpad(key_code: KeyCode, key: &Key) -> Option<TerminalKey> {
        bevy_key_to_terminal_key(key, key_code, None, &TerminalModifiers::default())
    }

    fn keypad(key: KeypadKey, text: Option<&str>) -> Option<TerminalKey> {
        Some(TerminalKey::Keypad {
            key,
            text: text.and_then(KeyText::new),
        })
    }

    /// Asserts the numpad digit and operator keys map to keypad keys that
    /// carry their text.
    ///
    /// Case: the user types `1` and `/` on the numeric keypad with Num Lock on.
    #[test]
    fn numpad_digits_map_to_keypad_keys_carrying_their_text() {
        assert_eq!(
            map_numpad(KeyCode::Numpad1, &Key::Character("1".into())),
            keypad(KeypadKey::One, Some("1"))
        );
        assert_eq!(
            map_numpad(KeyCode::NumpadDivide, &Key::Character("/".into())),
            keypad(KeypadKey::Divide, Some("/"))
        );
    }

    /// Asserts the numpad decimal and comma keys keep the character the
    /// layout gives them.
    ///
    /// Case: the user types on a keypad whose decimal key produces a comma.
    #[test]
    fn numpad_decimal_keeps_the_layouts_character() {
        assert_eq!(
            map_numpad(KeyCode::NumpadDecimal, &Key::Character(",".into())),
            keypad(KeypadKey::Decimal, Some(","))
        );
        assert_eq!(
            map_numpad(KeyCode::NumpadComma, &Key::Character(",".into())),
            keypad(KeypadKey::Separator, Some(","))
        );
    }

    /// Asserts numpad keys that Bevy reports as navigation keys map to the
    /// keypad navigation keys with no text.
    ///
    /// Case: the user presses the keypad's arrow, Home, Insert, and Delete
    /// functions with Num Lock off.
    #[test]
    fn numpad_navigation_maps_to_keypad_navigation_keys() {
        let cases = [
            (KeyCode::Numpad8, Key::ArrowUp, KeypadKey::Up),
            (KeyCode::Numpad7, Key::Home, KeypadKey::Home),
            (KeyCode::Numpad0, Key::Insert, KeypadKey::Insert),
            (KeyCode::NumpadDecimal, Key::Delete, KeypadKey::Delete),
        ];
        for (key_code, key, expected) in cases {
            assert_eq!(
                map_numpad(key_code, &key),
                keypad(expected, None),
                "{key:?}"
            );
        }
    }

    /// Asserts the numpad Enter and the unnamed middle key map to the keypad
    /// Enter and Begin keys.
    ///
    /// Case: the user presses keypad Enter, or keypad 5 with Num Lock off.
    #[test]
    fn numpad_enter_and_begin_map_to_their_keypad_keys() {
        assert_eq!(
            map_numpad(KeyCode::NumpadEnter, &Key::Enter),
            keypad(KeypadKey::Enter, None)
        );
        assert_eq!(
            map_numpad(KeyCode::Numpad5, &Key::Clear),
            keypad(KeypadKey::Begin, None)
        );
        assert_eq!(
            map_numpad(
                KeyCode::Numpad5,
                &Key::Unidentified(NativeKey::Unidentified)
            ),
            keypad(KeypadKey::Begin, None)
        );
    }

    /// Asserts a numpad key outside the keypad table maps as its logical key
    /// alone.
    ///
    /// Case: the user presses the numpad Backspace key found on some keypads.
    #[test]
    fn an_unlisted_numpad_key_maps_as_before() {
        assert_eq!(
            map_numpad(KeyCode::NumpadBackspace, &Key::Backspace),
            Some(TerminalKey::Backspace)
        );
    }

    /// Asserts a recorded un-shifted character rides on the character's text.
    ///
    /// Case: the user presses Shift+1, which types `!` on a US layout.
    #[test]
    fn a_recorded_unshifted_character_rides_on_the_text() {
        assert_eq!(
            bevy_key_to_terminal_key(
                &Key::Character("!".into()),
                KeyCode::Digit1,
                Some('1'),
                &TerminalModifiers::default()
            ),
            Some(TerminalKey::Character(
                KeyText::new("!").unwrap().with_unshifted('1')
            ))
        );
    }

    /// Asserts an unidentified key maps only when a modifier is held and its
    /// un-shifted character was recorded.
    ///
    /// Case: the user presses Ctrl+Alt with a key whose layout gives no
    /// logical key, such as one on a non-Latin layout.
    #[test]
    fn an_unidentified_key_maps_only_with_a_modifier_and_a_recorded_character() {
        let key = Key::Unidentified(NativeKey::Unidentified);
        let ctrl_alt = TerminalModifiers {
            ctrl: true,
            alt: true,
            ..TerminalModifiers::default()
        };
        let none = TerminalModifiers::default();
        assert_eq!(
            bevy_key_to_terminal_key(&key, KeyCode::KeyA, Some('a'), &ctrl_alt),
            Some(TerminalKey::Unidentified { unshifted: 'a' })
        );
        assert_eq!(
            bevy_key_to_terminal_key(&key, KeyCode::KeyA, Some('a'), &none),
            None
        );
        assert_eq!(
            bevy_key_to_terminal_key(&key, KeyCode::KeyA, None, &ctrl_alt),
            None
        );
    }

    /// Asserts printable characters map to `TerminalKey::Character`, wrapping a
    /// non-empty `KeyText` — ASCII and multibyte alike.
    ///
    /// Case: the user types `a` or an IME-composed `あ` and the key handler
    /// forwards it to the terminal.
    #[test]
    fn printable_char_maps_to_character() {
        assert_eq!(
            map_key(&Key::Character("a".into())),
            Some(TerminalKey::Character(KeyText::new("a").unwrap()))
        );
        assert_eq!(
            map_key(&Key::Character("あ".into())),
            Some(TerminalKey::Character(KeyText::new("あ").unwrap()))
        );
    }

    /// Asserts the space bar maps to `TerminalKey::Character(" ")`.
    ///
    /// Case: the user presses Space in the terminal.
    #[test]
    fn space_maps_to_character() {
        assert_eq!(
            map_key(&Key::Space),
            Some(TerminalKey::Character(KeyText::new(" ").unwrap()))
        );
    }

    /// Asserts Enter, Backspace, Tab, Escape, and Delete map to their
    /// `TerminalKey` variants.
    ///
    /// Case: the user presses the editing and confirmation keys in a shell.
    #[test]
    fn control_keys_map_correctly() {
        assert_eq!(map_key(&Key::Enter), Some(TerminalKey::Enter));
        assert_eq!(map_key(&Key::Backspace), Some(TerminalKey::Backspace));
        assert_eq!(map_key(&Key::Tab), Some(TerminalKey::Tab));
        assert_eq!(map_key(&Key::Escape), Some(TerminalKey::Escape));
        assert_eq!(
            map_key(&Key::Delete),
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
            map_key(&Key::ArrowUp),
            Some(TerminalKey::Cursor(CursorKey::Up))
        );
        assert_eq!(
            map_key(&Key::ArrowDown),
            Some(TerminalKey::Cursor(CursorKey::Down))
        );
        assert_eq!(
            map_key(&Key::ArrowLeft),
            Some(TerminalKey::Cursor(CursorKey::Left))
        );
        assert_eq!(
            map_key(&Key::ArrowRight),
            Some(TerminalKey::Cursor(CursorKey::Right))
        );
        assert_eq!(
            map_key(&Key::Home),
            Some(TerminalKey::Cursor(CursorKey::Home))
        );
        assert_eq!(
            map_key(&Key::End),
            Some(TerminalKey::Cursor(CursorKey::End))
        );
        assert_eq!(
            map_key(&Key::PageUp),
            Some(TerminalKey::Editing(EditingKey::PageUp))
        );
        assert_eq!(
            map_key(&Key::PageDown),
            Some(TerminalKey::Editing(EditingKey::PageDown))
        );
        assert_eq!(
            map_key(&Key::Insert),
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
                map_key(&key),
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
        assert_eq!(map_key(&Key::Shift), None);
        assert_eq!(map_key(&Key::Control), None);
        assert_eq!(map_key(&Key::Alt), None);
        assert_eq!(map_key(&Key::Super), None);
        assert_eq!(map_key(&Key::F13), None);
    }

    /// Asserts an empty character payload maps to `None` rather than a
    /// zero-length `TerminalKey::Character`, since `KeyText` cannot represent
    /// empty text.
    ///
    /// Case: a platform IME or compose sequence delivers a `Key::Character`
    /// event carrying an empty string.
    #[test]
    fn empty_character_text_maps_to_none() {
        assert_eq!(map_key(&Key::Character("".into())), None);
    }
}
