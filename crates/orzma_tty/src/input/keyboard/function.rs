//! The top-row function keys F1 to F12.

use super::TerminalModifiers;

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
    pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8> {
        todo!()
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
}
