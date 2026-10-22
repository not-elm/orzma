//! The numeric keypad's physical key vocabulary and its legacy encoding,
//! which sends what the key typed or else what its navigation namesake sends.

use super::{CursorKey, EditingKey, KeyText, TerminalModifiers};

/// A physical key on the PC-layout numeric keypad.
///
/// The variant names the key itself; the character the key types travels in
/// [`TerminalKey::Keypad`](super::TerminalKey::Keypad)'s `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeypadKey {
    Zero,
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    /// The decimal key, which types `.` or the layout's decimal separator.
    Decimal,
    Divide,
    Multiply,
    Subtract,
    Add,
    /// The keypad's own Enter key.
    Enter,
    /// The `=` key, which Macintosh and Sun keypads carry.
    Equal,
    /// The thousands-separator key, which types `,` or the layout's separator.
    Separator,
    /// The 4 key with Num Lock off.
    Left,
    /// The 6 key with Num Lock off.
    Right,
    /// The 8 key with Num Lock off.
    Up,
    /// The 2 key with Num Lock off.
    Down,
    /// The 9 key with Num Lock off.
    PageUp,
    /// The 3 key with Num Lock off.
    PageDown,
    /// The 7 key with Num Lock off.
    Home,
    /// The 1 key with Num Lock off.
    End,
    /// The 0 key with Num Lock off.
    Insert,
    /// The decimal key with Num Lock off.
    Delete,
    /// The 5 key with Num Lock off.
    Begin,
}

impl KeypadKey {
    /// Encodes the keypad key to the PTY bytes of the legacy form.
    ///
    /// With `text`, the key sends what a text key sends. Without it, Enter
    /// sends CR, the arrows, Home, and End send what their cursor keys send
    /// under `app_cursor` (DECCKM), and Insert, Delete, PageUp, and PageDown
    /// send what their editing keys send. Begin, a digit, and an operator key
    /// send nothing without `text`.
    pub(super) fn encode_legacy(
        self,
        text: Option<&KeyText>,
        mods: &TerminalModifiers,
        app_cursor: bool,
    ) -> Vec<u8> {
        if let Some(text) = text {
            return text.encode(mods);
        }
        match self {
            Self::Enter => vec![0x0d],
            Self::Up => CursorKey::Up.encode(mods, app_cursor),
            Self::Down => CursorKey::Down.encode(mods, app_cursor),
            Self::Left => CursorKey::Left.encode(mods, app_cursor),
            Self::Right => CursorKey::Right.encode(mods, app_cursor),
            Self::Home => CursorKey::Home.encode(mods, app_cursor),
            Self::End => CursorKey::End.encode(mods, app_cursor),
            Self::Insert => EditingKey::Insert.encode(mods),
            Self::Delete => EditingKey::Delete.encode(mods),
            Self::PageUp => EditingKey::PageUp.encode(mods),
            Self::PageDown => EditingKey::PageDown.encode(mods),
            Self::Zero
            | Self::One
            | Self::Two
            | Self::Three
            | Self::Four
            | Self::Five
            | Self::Six
            | Self::Seven
            | Self::Eight
            | Self::Nine
            | Self::Decimal
            | Self::Divide
            | Self::Multiply
            | Self::Subtract
            | Self::Add
            | Self::Equal
            | Self::Separator
            | Self::Begin => Vec::new(),
        }
    }

    /// Returns the key number and final byte the kitty keyboard protocol
    /// gives the key: 57399 to 57426 with `u`, and 1 with `E` for Begin.
    ///
    /// # References
    ///
    /// - <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>, "Functional
    ///   key definitions": `KP_0` "`57399 u`" through `KP_DELETE`
    ///   "`57426 u`", and `KP_BEGIN` "`1 E or 57427 ~`".
    pub(super) fn kitty_key(self) -> (u32, char) {
        let number = match self {
            Self::Zero => 57399,
            Self::One => 57400,
            Self::Two => 57401,
            Self::Three => 57402,
            Self::Four => 57403,
            Self::Five => 57404,
            Self::Six => 57405,
            Self::Seven => 57406,
            Self::Eight => 57407,
            Self::Nine => 57408,
            Self::Decimal => 57409,
            Self::Divide => 57410,
            Self::Multiply => 57411,
            Self::Subtract => 57412,
            Self::Add => 57413,
            Self::Enter => 57414,
            Self::Equal => 57415,
            Self::Separator => 57416,
            Self::Left => 57417,
            Self::Right => 57418,
            Self::Up => 57419,
            Self::Down => 57420,
            Self::PageUp => 57421,
            Self::PageDown => 57422,
            Self::Home => 57423,
            Self::End => 57424,
            Self::Insert => 57425,
            Self::Delete => 57426,
            Self::Begin => return (1, 'E'),
        };
        (number, 'u')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_mods() -> TerminalModifiers {
        TerminalModifiers::default()
    }

    /// Asserts that a keypad key carrying text sends what a text key sends,
    /// with Alt prefixing ESC.
    ///
    /// Case: a layout types a comma on the keypad's decimal key, and the user
    /// also types a digit with Alt held.
    #[test]
    fn keypad_text_is_sent_as_a_text_key_sends_it() {
        let one = KeyText::new("1").expect("one is non-empty");
        let comma = KeyText::new(",").expect("a comma is non-empty");
        let alt = TerminalModifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            KeypadKey::One.encode_legacy(Some(&one), &no_mods(), false),
            b"1".to_vec()
        );
        assert_eq!(
            KeypadKey::One.encode_legacy(Some(&one), &alt, false),
            b"\x1b1".to_vec()
        );
        assert_eq!(
            KeypadKey::Decimal.encode_legacy(Some(&comma), &no_mods(), false),
            b",".to_vec()
        );
    }

    /// Asserts that the keypad Enter without text sends CR whether or not
    /// Shift is held.
    ///
    /// Case: the user submits a shell line with the keypad Enter key.
    #[test]
    fn keypad_enter_without_text_sends_carriage_return() {
        let shift = TerminalModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            KeypadKey::Enter.encode_legacy(None, &no_mods(), false),
            b"\r".to_vec()
        );
        assert_eq!(
            KeypadKey::Enter.encode_legacy(None, &shift, false),
            b"\r".to_vec()
        );
    }

    /// Asserts that keypad navigation keys without text send what their
    /// cursor and editing namesakes send, honouring DECCKM and modifiers.
    ///
    /// Case: NumLock is off and the user moves through a document with the
    /// keypad's arrows, Home, Page Up, and Delete.
    #[test]
    fn keypad_navigation_keys_send_their_namesakes_legacy_form() {
        let ctrl = TerminalModifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            KeypadKey::Up.encode_legacy(None, &no_mods(), false),
            b"\x1b[A".to_vec()
        );
        assert_eq!(
            KeypadKey::Up.encode_legacy(None, &no_mods(), true),
            b"\x1bOA".to_vec()
        );
        assert_eq!(
            KeypadKey::Up.encode_legacy(None, &ctrl, false),
            b"\x1b[1;5A".to_vec()
        );
        assert_eq!(
            KeypadKey::Home.encode_legacy(None, &no_mods(), true),
            b"\x1bOH".to_vec()
        );
        assert_eq!(
            KeypadKey::PageUp.encode_legacy(None, &no_mods(), false),
            b"\x1b[5~".to_vec()
        );
        assert_eq!(
            KeypadKey::Delete.encode_legacy(None, &no_mods(), false),
            b"\x1b[3~".to_vec()
        );
    }

    /// Asserts that the keypad Begin key without text sends no bytes.
    ///
    /// Case: the user presses the keypad's centre key with NumLock off.
    #[test]
    fn keypad_begin_sends_nothing() {
        assert!(
            KeypadKey::Begin
                .encode_legacy(None, &no_mods(), false)
                .is_empty()
        );
    }

    /// Asserts that a keypad digit or operator key without text sends no
    /// bytes rather than the character it names.
    ///
    /// Case: on Windows, the user holds Alt and types an Alt code on the
    /// keypad, so each digit key arrives without the character it typed.
    #[test]
    fn a_keypad_digit_or_operator_without_text_sends_nothing() {
        let alt = TerminalModifiers {
            alt: true,
            ..Default::default()
        };
        assert!(KeypadKey::Seven.encode_legacy(None, &alt, false).is_empty());
        assert!(
            KeypadKey::Divide
                .encode_legacy(None, &alt, false)
                .is_empty()
        );
    }
}
