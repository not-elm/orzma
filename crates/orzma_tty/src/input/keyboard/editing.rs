//! The editing keypad keys Insert, Delete, PageUp, and PageDown.

use super::{TerminalModifiers, modified_sequence};

/// A key of the editing keypad: Insert, Delete, PageUp, or PageDown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditingKey {
    Insert,
    Delete,
    PageUp,
    PageDown,
}

impl EditingKey {
    /// Encodes the editing key to the pty input buffer.
    ///
    /// Without Shift, Alt, or Ctrl, Insert, Delete, PageUp, and PageDown send
    /// `CSI 2 ~`, `CSI 3 ~`, `CSI 5 ~`, and `CSI 6 ~`. With any of them held,
    /// the keys send `CSI n ; m ~` with the same `n`, where `m` is 1 plus
    /// Shift 1, Alt 2, and Ctrl 4. Meta does not count toward `m`. DECCKM
    /// does not affect these keys.
    ///
    /// # References
    ///
    /// - xterm-ctlseqs.pdf p.48, "PC-Style Function Keys".
    pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8> {
        let number: u8 = match self {
            Self::Insert => 2,
            Self::Delete => 3,
            Self::PageUp => 5,
            Self::PageDown => 6,
        };
        match mods.xterm_code() {
            Some(code) => modified_sequence(number, code, '~'),
            None => format!("\x1b[{number}~").into_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(shift: bool, alt: bool, ctrl: bool) -> TerminalModifiers {
        TerminalModifiers {
            shift,
            alt,
            ctrl,
            meta: false,
        }
    }

    fn numbers() -> [(EditingKey, u8); 4] {
        [
            (EditingKey::Insert, 2),
            (EditingKey::Delete, 3),
            (EditingKey::PageUp, 5),
            (EditingKey::PageDown, 6),
        ]
    }

    fn modifier_codes() -> [(TerminalModifiers, u8); 7] {
        [
            (held(true, false, false), 2),
            (held(false, true, false), 3),
            (held(true, true, false), 4),
            (held(false, false, true), 5),
            (held(true, false, true), 6),
            (held(false, true, true), 7),
            (held(true, true, true), 8),
        ]
    }

    /// Asserts that the editing keys without a modifier send `CSI 2 ~`,
    /// `CSI 3 ~`, `CSI 5 ~`, and `CSI 6 ~`.
    ///
    /// Case: the user presses Delete to remove the character under the cursor,
    /// or PageDown to scroll a pager, with no modifier key held.
    #[test]
    fn editing_keys_without_a_modifier_send_their_tilde_sequences() {
        let none = TerminalModifiers::default();
        let cases: [(EditingKey, &[u8]); 4] = [
            (EditingKey::Insert, b"\x1b[2~"),
            (EditingKey::Delete, b"\x1b[3~"),
            (EditingKey::PageUp, b"\x1b[5~"),
            (EditingKey::PageDown, b"\x1b[6~"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(&none), expected, "{key:?}");
        }
    }

    /// Asserts that the editing keys held with Shift, Alt, Control, or any
    /// combination of them send `CSI n ; m ~` with the key's own number, where
    /// `m` is the xterm modifier code from 2 to 8.
    ///
    /// Case: the user presses Ctrl-PageDown to switch to the next tab in an
    /// editor, or Ctrl-Delete to delete the next word.
    #[test]
    fn editing_keys_with_modifiers_append_the_modifier_code_as_a_parameter() {
        for (key, number) in numbers() {
            for (mods, code) in modifier_codes() {
                let expected = format!("\x1b[{number};{code}~");
                assert_eq!(
                    key.encode(&mods),
                    expected.as_bytes(),
                    "{key:?} with {mods:?}"
                );
            }
        }
    }

    /// Asserts that Meta is left out of the editing key modifier code, so
    /// Meta alone adds no parameter and Meta with other modifiers sends the
    /// code those modifiers give.
    ///
    /// Case: a caller hands the encoder an editing key with the Meta flag set,
    /// as a press with the Super key held would carry.
    #[test]
    fn meta_is_left_out_of_the_editing_key_modifier_code() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(EditingKey::Insert.encode(&meta), b"\x1b[2~");
        let meta_shift = TerminalModifiers {
            meta: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(EditingKey::PageUp.encode(&meta_shift), b"\x1b[5;2~");
        let meta_all = TerminalModifiers {
            meta: true,
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(EditingKey::PageDown.encode(&meta_all), b"\x1b[6;8~");
    }
}
