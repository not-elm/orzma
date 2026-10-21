//! The cursor keys: the four arrows, Home, and End.

use super::{TerminalModifiers, modified_sequence};

/// A cursor key: one of the four arrows, Home, or End.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKey {
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
}

impl CursorKey {
    /// Encodes the cursor key to the pty input buffer.
    ///
    /// Without Shift, Alt, or Ctrl, Up, Down, Right, Left, Home, and End send
    /// `CSI A`, `CSI B`, `CSI C`, `CSI D`, `CSI H`, and `CSI F`, or the same
    /// final characters after `SS3` when `app_cursor_keys` (DECCKM) is set.
    /// With any of them held, the keys send `CSI 1 ; m A` through
    /// `CSI 1 ; m F` whatever `app_cursor_keys` says, where `m` is 1 plus
    /// Shift 1, Alt 2, and Ctrl 4. Meta does not count toward `m`.
    ///
    /// # References
    ///
    /// - xterm-ctlseqs.pdf p.47, "PC-Style Function Keys".
    pub fn encode(self, mods: &TerminalModifiers, app_cursor_keys: bool) -> Vec<u8> {
        let final_byte = match self {
            Self::Up => 'A',
            Self::Down => 'B',
            Self::Right => 'C',
            Self::Left => 'D',
            Self::Home => 'H',
            Self::End => 'F',
        };
        match mods.xterm_code() {
            Some(code) => modified_sequence(1, code, final_byte),
            None if app_cursor_keys => format!("\x1bO{final_byte}").into_bytes(),
            None => format!("\x1b[{final_byte}").into_bytes(),
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

    fn final_bytes() -> [(CursorKey, char); 6] {
        [
            (CursorKey::Up, 'A'),
            (CursorKey::Down, 'B'),
            (CursorKey::Right, 'C'),
            (CursorKey::Left, 'D'),
            (CursorKey::Home, 'H'),
            (CursorKey::End, 'F'),
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

    /// Asserts that the cursor keys without a modifier send `CSI A` through
    /// `CSI D`, `CSI H`, and `CSI F` when DECCKM is reset.
    ///
    /// Case: the user moves through a shell's command line with the arrow
    /// keys, Home, and End, with no modifier key held.
    #[test]
    fn cursor_keys_without_a_modifier_send_csi_sequences_in_normal_mode() {
        let none = TerminalModifiers::default();
        let cases: [(CursorKey, &[u8]); 6] = [
            (CursorKey::Up, b"\x1b[A"),
            (CursorKey::Down, b"\x1b[B"),
            (CursorKey::Right, b"\x1b[C"),
            (CursorKey::Left, b"\x1b[D"),
            (CursorKey::Home, b"\x1b[H"),
            (CursorKey::End, b"\x1b[F"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(&none, false), expected, "{key:?}");
        }
    }

    /// Asserts that the cursor keys without a modifier send `SS3 A` through
    /// `SS3 D`, `SS3 H`, and `SS3 F` when DECCKM is set.
    ///
    /// Case: a full-screen editor turns on application cursor keys, and the
    /// user moves through the buffer with the arrow keys, Home, and End.
    #[test]
    fn cursor_keys_without_a_modifier_send_ss3_sequences_in_application_mode() {
        let none = TerminalModifiers::default();
        let cases: [(CursorKey, &[u8]); 6] = [
            (CursorKey::Up, b"\x1bOA"),
            (CursorKey::Down, b"\x1bOB"),
            (CursorKey::Right, b"\x1bOC"),
            (CursorKey::Left, b"\x1bOD"),
            (CursorKey::Home, b"\x1bOH"),
            (CursorKey::End, b"\x1bOF"),
        ];
        for (key, expected) in cases {
            assert_eq!(key.encode(&none, true), expected, "{key:?}");
        }
    }

    /// Asserts that the cursor keys held with Shift, Alt, Control, or any
    /// combination of them send `CSI 1 ; m` with the key's final character
    /// when DECCKM is reset, where `m` is the xterm modifier code from 2 to 8.
    ///
    /// Case: the user presses Ctrl-Left to move back one word in a shell, or
    /// Shift-End to select to the end of the line.
    #[test]
    fn cursor_keys_with_modifiers_send_csi_1_with_the_modifier_code_in_normal_mode() {
        for (key, final_byte) in final_bytes() {
            for (mods, code) in modifier_codes() {
                let expected = format!("\x1b[1;{code}{final_byte}");
                assert_eq!(
                    key.encode(&mods, false),
                    expected.as_bytes(),
                    "{key:?} with {mods:?}"
                );
            }
        }
    }

    /// Asserts that the cursor keys held with a modifier send the same
    /// `CSI 1 ; m` sequences when DECCKM is set, rather than switching to
    /// `SS3`.
    ///
    /// Case: the user presses Ctrl-Right or Shift-Up in a full-screen editor
    /// that has turned on application cursor keys.
    #[test]
    fn cursor_keys_with_modifiers_keep_the_csi_form_in_application_mode() {
        for (key, final_byte) in final_bytes() {
            for (mods, code) in modifier_codes() {
                let expected = format!("\x1b[1;{code}{final_byte}");
                assert_eq!(
                    key.encode(&mods, true),
                    expected.as_bytes(),
                    "{key:?} with {mods:?}"
                );
            }
        }
    }

    /// Asserts that Meta is left out of the cursor key modifier code, so Meta
    /// alone keeps the unmodified sequence in either DECCKM mode and Meta with
    /// other modifiers sends the code those modifiers give.
    ///
    /// Case: a caller hands the encoder a cursor key with the Meta flag set,
    /// as a press with the Super key held would carry.
    #[test]
    fn meta_is_left_out_of_the_cursor_key_modifier_code() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(CursorKey::Up.encode(&meta, false), b"\x1b[A");
        assert_eq!(CursorKey::Up.encode(&meta, true), b"\x1bOA");
        let meta_shift = TerminalModifiers {
            meta: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(CursorKey::Up.encode(&meta_shift, false), b"\x1b[1;2A");
        let meta_all = TerminalModifiers {
            meta: true,
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(CursorKey::End.encode(&meta_all, true), b"\x1b[1;8F");
    }
}
