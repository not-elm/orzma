//! The numeric keypad's key vocabulary and its VT encoder. Which bytes a
//! keypad key sends depends on `DECKPAM` / `DECKPNM` rather than on the
//! modifiers the rest of the keyboard reads, so the encoding lives here
//! beside the vocabulary it indexes.

use orzma_vt::prelude::KeypadMode;

/// A key on the PC-layout numeric keypad.
///
/// [`Self::Comma`] and [`Self::Decimal`] name the character a key types
/// rather than the station it sits at, which is how xterm keys its keypad
/// table. A layout whose decimal separator is a comma therefore reaches this
/// vocabulary as [`Self::Comma`].
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
    /// The neighbouring [PC-Style Function Keys] table is not this function's
    /// specification. Its application column lists the editing sequences a
    /// station emits when NumLock is off, which reach a terminal as separate
    /// keys rather than as a keypad mode, so deriving from it turns keypad
    /// digits into cursor keys.
    ///
    /// # References
    ///
    /// - [VT220-Style Function Keys] — the keypad table this follows.
    ///
    /// [VT220-Style Function Keys]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-VT220-Style-Function-Keys
    /// [PC-Style Function Keys]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-PC-Style-Function-Keys
    pub(super) fn encode(&self, mode: KeypadMode) -> &'static [u8] {
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
            KeypadMode::Application => {
                todo!()
            }
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
}
