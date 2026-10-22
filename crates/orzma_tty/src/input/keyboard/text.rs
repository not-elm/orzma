//! Typed text, and the bytes Ctrl and Alt make it send.

use super::TerminalModifiers;

/// Non-empty UTF-8 text carried by
/// [`TerminalKey::Character`](super::TerminalKey::Character), with the
/// character its key types when no modifier is held, if the host recorded
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyText {
    text: String,
    unshifted: Option<char>,
}

impl KeyText {
    /// Wraps `text`, returning `None` when it is empty.
    ///
    /// No un-shifted key is recorded.
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.is_empty()).then_some(Self {
            text,
            unshifted: None,
        })
    }

    /// Returns the text with `unshifted` recorded as the character its key
    /// types when no modifier is held.
    ///
    /// The kitty keyboard protocol's form numbers a one-character text by
    /// that character.
    pub fn with_unshifted(self, unshifted: char) -> Self {
        Self {
            unshifted: Some(unshifted),
            ..self
        }
    }

    /// Returns the wrapped text, which is never empty.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Encodes the text to the pty input buffer.
    ///
    /// With Ctrl held, a single character that has a control character sends
    /// that byte instead. A letter in either case sends its C0 byte, so `a`
    /// and `A` both send SOH. `Space`, `@`, and `2` send NUL; `[` and `3` send
    /// ESC; `\` and `4` send FS; `]` and `5` send GS; `^`, `~`, and `6` send
    /// RS; `_`, `/`, and `7` send US; and `?` and `8` send DEL. With Alt or
    /// Meta also held, `ESC` comes before that byte. Any other text is sent
    /// as UTF-8, after `ESC` when Alt or Meta is held.
    pub fn encode(&self, mods: &TerminalModifiers) -> Vec<u8> {
        let control = if mods.ctrl { self.control_byte() } else { None };
        let mut out = Vec::new();
        if mods.alt || mods.meta {
            out.push(0x1b);
        }
        match control {
            Some(byte) => out.push(byte),
            None => out.extend_from_slice(self.text.as_bytes()),
        }
        out
    }

    /// Returns the key number the kitty keyboard protocol gives the key that
    /// typed the text, or `None` when the text is more than one character.
    ///
    /// The number is the code point of the recorded un-shifted key. Without
    /// one, it is the code point of the character lowercased when its
    /// lowercase is one character, and of the character itself otherwise.
    pub(super) fn key_number(&self) -> Option<u32> {
        let c = self.single_char()?;
        let number = self.unshifted.unwrap_or_else(|| {
            let mut lowercase = c.to_lowercase();
            match (lowercase.next(), lowercase.next()) {
                (Some(lower), None) => lower,
                _ => c,
            }
        });
        Some(u32::from(number))
    }

    /// Returns the control character Ctrl turns the text into, or `None`
    /// when the text is not a single character that has one.
    fn control_byte(&self) -> Option<u8> {
        let c = self.single_char()?;
        match c {
            'a'..='z' | 'A'..='Z' => Some(c.to_ascii_uppercase() as u8 - b'@'),
            ' ' | '@' | '2' => Some(0x00),
            '[' | '3' => Some(0x1b),
            '\\' | '4' => Some(0x1c),
            ']' | '5' => Some(0x1d),
            '^' | '~' | '6' => Some(0x1e),
            '_' | '/' | '7' => Some(0x1f),
            '?' | '8' => Some(0x7f),
            _ => None,
        }
    }

    /// Returns the text's character, or `None` when the text is more than
    /// one character.
    fn single_char(&self) -> Option<char> {
        let mut chars = self.text.chars();
        let c = chars.next()?;
        chars.next().is_none().then_some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> KeyText {
        KeyText::new(s).expect("test text must be non-empty")
    }

    fn held(ctrl: bool, alt: bool, meta: bool) -> TerminalModifiers {
        TerminalModifiers {
            ctrl,
            shift: false,
            alt,
            meta,
        }
    }

    fn ctrl() -> TerminalModifiers {
        held(true, false, false)
    }

    fn alt() -> TerminalModifiers {
        held(false, true, false)
    }

    fn meta() -> TerminalModifiers {
        held(false, false, true)
    }

    fn alt_meta() -> TerminalModifiers {
        held(false, true, true)
    }

    fn ctrl_alt() -> TerminalModifiers {
        held(true, true, false)
    }

    fn ctrl_meta() -> TerminalModifiers {
        held(true, false, true)
    }

    /// Asserts that Ctrl with a letter of either case sends that letter's C0
    /// byte, from SOH for `a` to SUB for `z`.
    ///
    /// Case: the user presses Ctrl-C to interrupt a command, or Ctrl-Shift-A in
    /// a program that receives the letter in uppercase.
    #[test]
    fn ctrl_with_a_letter_sends_its_c0_byte_in_either_case() {
        for (byte, lower) in (1u8..).zip('a'..='z') {
            let upper = lower.to_ascii_uppercase();
            assert_eq!(text(&lower.to_string()).encode(&ctrl()), [byte], "{lower}");
            assert_eq!(text(&upper.to_string()).encode(&ctrl()), [byte], "{upper}");
        }
    }

    /// Asserts that Ctrl with `Space`, `2` to `8`, `[`, `]`, or `~` sends the
    /// control character the VT220 assigns to that key.
    ///
    /// Case: the user presses Ctrl-Space to set the mark in Emacs, Ctrl-[ to
    /// leave insert mode in vim, or Ctrl-6 to switch to the alternate file.
    #[test]
    fn ctrl_with_a_vt220_control_key_sends_its_control_character() {
        let cases: [(&str, u8); 11] = [
            (" ", 0x00),
            ("2", 0x00),
            ("3", 0x1b),
            ("[", 0x1b),
            ("4", 0x1c),
            ("5", 0x1d),
            ("]", 0x1d),
            ("6", 0x1e),
            ("~", 0x1e),
            ("7", 0x1f),
            ("8", 0x7f),
        ];
        for (key, byte) in cases {
            assert_eq!(text(key).encode(&ctrl()), [byte], "{key:?}");
        }
    }

    /// Asserts that Alt or Meta held without Ctrl sends `ESC` before an
    /// ordinary character, and that the character alone is sent unchanged.
    ///
    /// Case: the user presses Alt-b to move back one word in a shell, or types
    /// a plain letter with no modifier held.
    #[test]
    fn alt_or_meta_without_ctrl_prefixes_an_ordinary_character_with_esc() {
        assert_eq!(text("x").encode(&TerminalModifiers::default()), b"x");
        assert_eq!(text("x").encode(&meta()), b"\x1bx");
        assert_eq!(text("x").encode(&alt()), b"\x1bx");
        assert_eq!(text("x").encode(&alt_meta()), b"\x1bx");
    }

    /// Asserts that Ctrl with `/` sends US and Ctrl with `?` sends DEL, rather
    /// than the FS and US the VT220 table assigns to those keys.
    ///
    /// Case: the user presses Ctrl-/ to undo in Emacs, or Ctrl-? in a program
    /// that reads DEL as Backspace.
    #[test]
    fn ctrl_with_slash_sends_us_and_ctrl_with_question_mark_sends_del() {
        assert_eq!(text("/").encode(&ctrl()), [0x1f]);
        assert_eq!(text("?").encode(&ctrl()), [0x7f]);
    }

    /// Asserts that Ctrl with `@`, `\`, `^`, or `_` sends NUL, FS, RS, or US.
    ///
    /// Case: the user presses Ctrl-\ to send SIGQUIT, Ctrl-_ to undo in
    /// readline, or Ctrl-^ to switch to the alternate file in vim.
    #[test]
    fn ctrl_with_an_ascii_control_symbol_sends_its_control_character() {
        let cases: [(&str, u8); 4] = [("@", 0x00), ("\\", 0x1c), ("^", 0x1e), ("_", 0x1f)];
        for (key, byte) in cases {
            assert_eq!(text(key).encode(&ctrl()), [byte], "{key:?}");
        }
    }

    /// Asserts that Ctrl with Alt or Meta sends `ESC` before the control
    /// character Ctrl alone sends.
    ///
    /// Case: the user presses Ctrl-Alt-f to move forward over an expression in
    /// Emacs, or Ctrl-Alt-h to delete the previous word in readline.
    #[test]
    fn ctrl_with_alt_or_meta_prefixes_the_control_character_with_esc() {
        let cases: [(&str, &[u8]); 6] = [
            ("f", b"\x1b\x06"),
            ("F", b"\x1b\x06"),
            (" ", b"\x1b\x00"),
            ("[", b"\x1b\x1b"),
            ("?", b"\x1b\x7f"),
            ("8", b"\x1b\x7f"),
        ];
        for (key, expected) in cases {
            assert_eq!(text(key).encode(&ctrl_alt()), expected, "{key:?}");
        }
        assert_eq!(text("a").encode(&ctrl_meta()), b"\x1b\x01");
    }

    /// Asserts that Ctrl with text that has no control character sends the
    /// text unchanged, after `ESC` when Alt is also held.
    ///
    /// Case: the user presses Ctrl-1, Ctrl-9, or Ctrl-- in a shell, keys that
    /// have no control character of their own.
    #[test]
    fn ctrl_with_text_that_has_no_control_character_sends_the_text() {
        for key in ["1", "9", "0", "-", "あ", "ab"] {
            assert_eq!(text(key).encode(&ctrl()), key.as_bytes(), "{key:?}");
        }
        assert_eq!(text("1").encode(&ctrl_alt()), b"\x1b1");
    }

    /// Asserts that non-ASCII text is sent as UTF-8, after `ESC` when Alt is
    /// held.
    ///
    /// Case: the user types `あ` or a composed `é` through an input method, or
    /// presses Alt with a key that types a non-ASCII letter.
    #[test]
    fn non_ascii_text_is_sent_as_utf8() {
        for key in ["あ", "e\u{301}"] {
            assert_eq!(
                text(key).encode(&TerminalModifiers::default()),
                key.as_bytes(),
                "{key:?}"
            );
        }
        let mut expected = vec![0x1b];
        expected.extend_from_slice("あ".as_bytes());
        assert_eq!(text("あ").encode(&alt()), expected);
    }

    /// Asserts that empty text is rejected.
    ///
    /// Case: a platform IME delivers a key event whose text is empty.
    #[test]
    fn key_text_rejects_empty() {
        assert_eq!(KeyText::new(""), None);
    }

    /// Asserts that non-empty text is kept as given.
    ///
    /// Case: the user types the letter `a`.
    #[test]
    fn key_text_keeps_non_empty_input() {
        assert_eq!(
            KeyText::new("a").map(|t| t.as_str().to_owned()),
            Some("a".to_owned())
        );
    }
}
