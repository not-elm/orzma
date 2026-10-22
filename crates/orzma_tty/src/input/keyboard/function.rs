//! The top-row function keys F1 to F12.

use super::{TerminalModifiers, modified_sequence};

/// A top-row function key, F1 through F12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKey {
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl FunctionKey {
    /// Encodes the function key to the pty input buffer.
    ///
    /// Without Shift, Alt, or Ctrl, F1 to F4 send `SS3 P` to `SS3 S` and F5
    /// to F12 send `CSI n ~`. With any of them held, the xterm modifier code
    /// becomes the second parameter: F1 to F4 send `CSI 1 ; m P` to
    /// `CSI 1 ; m S`, and F5 to F12 send `CSI n ; m ~`. Meta does not count
    /// toward the code.
    ///
    /// # References
    ///
    /// - xterm-ctlseqs.pdf p.47-48, "PC-Style Function Keys".
    pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8> {
        let (number, final_byte): (u8, char) = match self {
            Self::F1 => (1, 'P'),
            Self::F2 => (1, 'Q'),
            Self::F3 => (1, 'R'),
            Self::F4 => (1, 'S'),
            Self::F5 => (15, '~'),
            Self::F6 => (17, '~'),
            Self::F7 => (18, '~'),
            Self::F8 => (19, '~'),
            Self::F9 => (20, '~'),
            Self::F10 => (21, '~'),
            Self::F11 => (23, '~'),
            Self::F12 => (24, '~'),
        };
        match (mods.xterm_code(), final_byte) {
            (Some(code), _) => modified_sequence(number, code, final_byte),
            (None, '~') => format!("\x1b[{number}~").into_bytes(),
            (None, _) => format!("\x1bO{final_byte}").into_bytes(),
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

    /// Asserts that F5 through F12 without a modifier send `CSI n ~` with
    /// each key's own number, skipping 16 and 22.
    ///
    /// Case: the user presses F10 to quit htop, or F5 to copy a file in
    /// Midnight Commander, with no modifier key held.
    #[test]
    fn f5_through_f12_without_a_modifier_send_their_tilde_sequences() {
        let none = TerminalModifiers::default();
        let cases: [(FunctionKey, &[u8]); 8] = [
            (FunctionKey::F5, b"\x1b[15~"),
            (FunctionKey::F6, b"\x1b[17~"),
            (FunctionKey::F7, b"\x1b[18~"),
            (FunctionKey::F8, b"\x1b[19~"),
            (FunctionKey::F9, b"\x1b[20~"),
            (FunctionKey::F10, b"\x1b[21~"),
            (FunctionKey::F11, b"\x1b[23~"),
            (FunctionKey::F12, b"\x1b[24~"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(&none), expected, "{key:?}");
        }
    }

    /// Asserts that F5 through F12 held with Shift, Alt, Control, or any
    /// combination of them send `CSI n ; m ~`, where `m` is the xterm
    /// modifier code from 2 to 8.
    ///
    /// Case: the user presses Shift-F5, Ctrl-F7, or Ctrl-Alt-F12 in a
    /// full-screen editor that binds the modified function keys.
    #[test]
    fn f5_through_f12_with_modifiers_append_the_modifier_code_as_a_parameter() {
        let keys: [(FunctionKey, u8); 8] = [
            (FunctionKey::F5, 15),
            (FunctionKey::F6, 17),
            (FunctionKey::F7, 18),
            (FunctionKey::F8, 19),
            (FunctionKey::F9, 20),
            (FunctionKey::F10, 21),
            (FunctionKey::F11, 23),
            (FunctionKey::F12, 24),
        ];
        let modifiers: [(TerminalModifiers, u8); 7] = [
            (held(true, false, false), 2),
            (held(false, true, false), 3),
            (held(true, true, false), 4),
            (held(false, false, true), 5),
            (held(true, false, true), 6),
            (held(false, true, true), 7),
            (held(true, true, true), 8),
        ];
        for (key, number) in keys {
            for (mods, code) in modifiers {
                let expected = format!("\x1b[{number};{code}~");
                assert_eq!(
                    key.encode(&mods),
                    expected.as_bytes(),
                    "{key:?} with {mods:?}"
                );
            }
        }
        assert_eq!(
            FunctionKey::F5.encode(&held(true, false, false)),
            b"\x1b[15;2~"
        );
    }

    /// Asserts that F1 through F4 without a modifier send `SS3 P` through
    /// `SS3 S`.
    ///
    /// Case: the user presses F1 to open the help screen of htop or Midnight
    /// Commander, with no modifier key held.
    #[test]
    fn f1_through_f4_without_a_modifier_send_their_ss3_sequences() {
        let none = TerminalModifiers::default();
        let cases: [(FunctionKey, &[u8]); 4] = [
            (FunctionKey::F1, b"\x1bOP"),
            (FunctionKey::F2, b"\x1bOQ"),
            (FunctionKey::F3, b"\x1bOR"),
            (FunctionKey::F4, b"\x1bOS"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(&none), expected, "{key:?}");
        }
    }

    /// Asserts that F1 through F4 held with Shift, Alt, Control, or any
    /// combination of them switch from `SS3` to `CSI 1 ; m` with the same
    /// final character, where `m` is the xterm modifier code from 2 to 8.
    ///
    /// Case: the user presses Shift-F1, Ctrl-F3, or Ctrl-Alt-F4 in a
    /// full-screen editor that binds the modified function keys.
    #[test]
    fn f1_through_f4_with_modifiers_switch_from_ss3_to_csi_with_the_modifier_code() {
        let keys: [(FunctionKey, char); 4] = [
            (FunctionKey::F1, 'P'),
            (FunctionKey::F2, 'Q'),
            (FunctionKey::F3, 'R'),
            (FunctionKey::F4, 'S'),
        ];
        let modifiers: [(TerminalModifiers, u8); 7] = [
            (held(true, false, false), 2),
            (held(false, true, false), 3),
            (held(true, true, false), 4),
            (held(false, false, true), 5),
            (held(true, false, true), 6),
            (held(false, true, true), 7),
            (held(true, true, true), 8),
        ];
        for (key, final_byte) in keys {
            for (mods, code) in modifiers {
                let expected = format!("\x1b[1;{code}{final_byte}");
                assert_eq!(
                    key.encode(&mods),
                    expected.as_bytes(),
                    "{key:?} with {mods:?}"
                );
            }
        }
        assert_eq!(
            FunctionKey::F3.encode(&held(true, false, false)),
            b"\x1b[1;2R"
        );
        assert_eq!(
            FunctionKey::F3.encode(&held(false, false, true)),
            b"\x1b[1;5R"
        );
        assert_eq!(
            FunctionKey::F3.encode(&held(true, false, true)),
            b"\x1b[1;6R"
        );
    }

    /// Asserts that Meta is left out of the function key modifier code, so
    /// Meta alone adds no parameter and Meta with other modifiers sends the
    /// code those modifiers give.
    ///
    /// Case: a caller hands the encoder a function key with the Meta flag
    /// set, as a press of F5 or F12 with the Super key held would carry.
    #[test]
    fn meta_is_left_out_of_the_function_key_modifier_code() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(FunctionKey::F5.encode(&meta), b"\x1b[15~");
        let meta_shift = TerminalModifiers {
            meta: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(FunctionKey::F5.encode(&meta_shift), b"\x1b[15;2~");
        let meta_all = TerminalModifiers {
            meta: true,
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(FunctionKey::F12.encode(&meta_all), b"\x1b[24;8~");
    }

    /// Asserts that Meta is left out of the modifier code of F1 through F4,
    /// so Meta alone keeps the `SS3` form and Meta with other modifiers sends
    /// the `CSI` form those modifiers give.
    ///
    /// Case: a caller hands the encoder F1 or F4 with the Meta flag set, as a
    /// press with the Super key held would carry.
    #[test]
    fn meta_is_left_out_of_the_modifier_code_of_f1_through_f4() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(FunctionKey::F1.encode(&meta), b"\x1bOP");
        let meta_shift = TerminalModifiers {
            meta: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(FunctionKey::F1.encode(&meta_shift), b"\x1b[1;2P");
        let meta_all = TerminalModifiers {
            meta: true,
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(FunctionKey::F4.encode(&meta_all), b"\x1b[1;8S");
    }
}
