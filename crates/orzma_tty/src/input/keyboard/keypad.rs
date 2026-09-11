//! The numeric keypad's key vocabulary and its VT encoder. Which bytes a
//! keypad key sends depends on `DECKPAM` / `DECKPNM` rather than on the
//! modifiers the rest of the keyboard reads.

use orzma_vt::prelude::KeypadMode;

/// A key on the PC-layout numeric keypad.
///
/// [`Self::Comma`] and [`Self::Decimal`] name the character a key types
/// rather than the station it sits at, so a layout whose decimal separator
/// is a comma reaches this vocabulary as [`Self::Comma`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeypadKey {
    Divide,
    Multiply,
    Subtract,
    Add,
    /// The `,` key, which German and French layouts type where a US layout
    /// types [`Self::Decimal`].
    Comma,
    /// The `=` key, which Macintosh and Sun keypads carry.
    Equal,
    /// The `.` key.
    Decimal,
    Enter,
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
}

impl KeypadKey {
    /// Encodes the keypad key input to the PTY bytes.
    ///
    /// # References
    ///
    /// - [VT220-Style Function Keys] — the keypad table this follows.
    ///
    /// [VT220-Style Function Keys]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-VT220-Style-Function-Keys
    pub(super) fn encode(&self, mode: KeypadMode) -> &'static [u8] {
        macro_rules! ss3 {
            ($numeric:literal) => {
                &[0x1b, b'O', $numeric + 0x40]
            };
        }
        match mode {
            KeypadMode::Numeric => match self {
                KeypadKey::Divide => b"/",
                KeypadKey::Multiply => b"*",
                KeypadKey::Subtract => b"-",
                KeypadKey::Add => b"+",
                KeypadKey::Comma => b",",
                KeypadKey::Equal => b"=",
                KeypadKey::Decimal => b".",
                KeypadKey::Enter => b"\r",
                KeypadKey::Zero => b"0",
                KeypadKey::One => b"1",
                KeypadKey::Two => b"2",
                KeypadKey::Three => b"3",
                KeypadKey::Four => b"4",
                KeypadKey::Five => b"5",
                KeypadKey::Six => b"6",
                KeypadKey::Seven => b"7",
                KeypadKey::Eight => b"8",
                KeypadKey::Nine => b"9",
            },
            KeypadMode::Application => match self {
                KeypadKey::Divide => ss3!(b'/'),
                KeypadKey::Multiply => ss3!(b'*'),
                KeypadKey::Subtract => ss3!(b'-'),
                KeypadKey::Add => ss3!(b'+'),
                KeypadKey::Comma => ss3!(b','),
                KeypadKey::Equal => b"\x1bOX",
                KeypadKey::Decimal => ss3!(b'.'),
                KeypadKey::Enter => ss3!(b'\r'),
                KeypadKey::Zero => ss3!(b'0'),
                KeypadKey::One => ss3!(b'1'),
                KeypadKey::Two => ss3!(b'2'),
                KeypadKey::Three => ss3!(b'3'),
                KeypadKey::Four => ss3!(b'4'),
                KeypadKey::Five => ss3!(b'5'),
                KeypadKey::Six => ss3!(b'6'),
                KeypadKey::Seven => ss3!(b'7'),
                KeypadKey::Eight => ss3!(b'8'),
                KeypadKey::Nine => ss3!(b'9'),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that numeric mode sends the character each keypad key types
    /// rather than an escape sequence.
    ///
    /// Case: a shell prompt is waiting for input and the user types a figure
    /// and an operator on the numeric keypad, then presses its Enter to
    /// submit the line.
    #[test]
    fn numeric_mode_sends_characters_rather_than_sequences() {
        let cases: [(KeypadKey, &[u8]); 18] = [
            (KeypadKey::Divide, b"/"),
            (KeypadKey::Multiply, b"*"),
            (KeypadKey::Subtract, b"-"),
            (KeypadKey::Add, b"+"),
            (KeypadKey::Comma, b","),
            (KeypadKey::Equal, b"="),
            (KeypadKey::Decimal, b"."),
            (KeypadKey::Enter, b"\r"),
            (KeypadKey::Zero, b"0"),
            (KeypadKey::One, b"1"),
            (KeypadKey::Two, b"2"),
            (KeypadKey::Three, b"3"),
            (KeypadKey::Four, b"4"),
            (KeypadKey::Five, b"5"),
            (KeypadKey::Six, b"6"),
            (KeypadKey::Seven, b"7"),
            (KeypadKey::Eight, b"8"),
            (KeypadKey::Nine, b"9"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(KeypadMode::Numeric), expected);
        }
    }

    /// Asserts that every keypad key except `=` sends `SS3` followed by its
    /// numeric-mode byte raised by 0x40, rather than the editing sequences
    /// the digits' gray legends name.
    ///
    /// Case: a full-screen spreadsheet has taken the keypad over with
    /// `DECKPAM` and the user types a figure and an operator into a cell,
    /// then presses the keypad Enter to commit it.
    #[test]
    fn application_mode_shifts_the_numeric_byte_into_the_ss3_range() {
        let cases: [(KeypadKey, &[u8]); 17] = [
            (KeypadKey::Enter, b"\x1bOM"),
            (KeypadKey::Multiply, b"\x1bOj"),
            (KeypadKey::Add, b"\x1bOk"),
            (KeypadKey::Comma, b"\x1bOl"),
            (KeypadKey::Subtract, b"\x1bOm"),
            (KeypadKey::Decimal, b"\x1bOn"),
            (KeypadKey::Divide, b"\x1bOo"),
            (KeypadKey::Zero, b"\x1bOp"),
            (KeypadKey::One, b"\x1bOq"),
            (KeypadKey::Two, b"\x1bOr"),
            (KeypadKey::Three, b"\x1bOs"),
            (KeypadKey::Four, b"\x1bOt"),
            (KeypadKey::Five, b"\x1bOu"),
            (KeypadKey::Six, b"\x1bOv"),
            (KeypadKey::Seven, b"\x1bOw"),
            (KeypadKey::Eight, b"\x1bOx"),
            (KeypadKey::Nine, b"\x1bOy"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(KeypadMode::Application), expected);
        }
    }

    /// Asserts that the keypad `=` uses `SS3 X`, which the character-plus-0x40
    /// rule the other keys follow would not produce.
    ///
    /// Case: a Macintosh keyboard, whose keypad carries `=`, is driving an
    /// application that has taken the keypad over.
    #[test]
    fn application_mode_equal_uses_ss3_x() {
        let expected: &[u8] = b"\x1bOX";
        assert_eq!(KeypadKey::Equal.encode(KeypadMode::Application), expected);
    }
}
