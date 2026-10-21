//! Pure VT-encoder for key input: translates a logical key and its
//! modifiers into the byte sequence the PTY expects.

mod cursor;
mod editing;
mod function;
mod keypad;

pub use cursor::CursorKey;
pub use editing::EditingKey;
pub use function::FunctionKey;
pub use keypad::KeypadKey;
use orzma_vt::prelude::KeypadMode;

/// Non-empty UTF-8 text carried by [`TerminalKey::Character`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyText(String);

impl KeyText {
    /// Wraps `text`, returning `None` when it is empty.
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.is_empty()).then_some(Self(text))
    }

    /// Returns the wrapped text, which is never empty.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Subset of keys the terminal input codec understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalKey {
    /// UTF-8 text (single char or multi-codepoint composition).
    Character(KeyText),
    Keypad(KeypadKey),
    Function(FunctionKey),
    Cursor(CursorKey),
    Editing(EditingKey),
    Enter,
    Backspace,
    Tab,
    Escape,
}

/// Modifier flags carried alongside `TerminalKey`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl TerminalModifiers {
    /// Returns the xterm modifier code a modified key carries as a
    /// parameter, 1 plus Shift 1, Alt 2, and Ctrl 4, or `None` when none of
    /// the three is held. Meta does not count toward the code.
    fn xterm_code(&self) -> Option<u8> {
        let code = 1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl);
        (code > 1).then_some(code)
    }
}

/// Translates a logical key + modifiers into the bytes the PTY expects.
///
/// The encoding is total: every `TerminalKey` has a representation.
///
/// The cursor keys — the arrows plus Home and End — honour
/// `app_cursor_keys` (DECCKM) when neither Shift, Alt, nor Ctrl is held:
/// `ESC [ A/B/C/D/H/F` in normal mode, `ESC O A/B/C/D/H/F` in application
/// mode. With any of the three held, they send `CSI 1 ; m A/B/C/D/H/F` in
/// either mode. The editing keypad (Insert, Delete, PageUp, PageDown) is
/// unaffected by DECCKM: it sends `CSI n ~`, or `CSI n ; m ~` with a
/// modifier held.
///
/// Tab sends HT, and CBT (`CSI Z`, the terminfo `kcbt` string) when Shift
/// is the only modifier held; other modifiers leave Tab as HT.
pub(super) fn encode_key(
    key: &TerminalKey,
    mods: &TerminalModifiers,
    app_cursor_keys: bool,
    keypad_mode: KeypadMode,
) -> Vec<u8> {
    match key {
        TerminalKey::Character(text) => encode_character(text, mods),
        TerminalKey::Keypad(keypad) => keypad.encode(keypad_mode).to_vec(),
        TerminalKey::Function(function) => function.encode(mods),
        TerminalKey::Cursor(cursor) => cursor.encode(mods, app_cursor_keys),
        TerminalKey::Editing(editing) => editing.encode(mods),
        TerminalKey::Enter => vec![0x0d],
        TerminalKey::Backspace => vec![0x7f],
        TerminalKey::Tab if mods.shift && !mods.ctrl && !mods.alt && !mods.meta => {
            b"\x1b[Z".to_vec()
        }
        TerminalKey::Tab => vec![0x09],
        TerminalKey::Escape => vec![0x1b],
    }
}

/// Priority order: Ctrl + ASCII letter collapses to a C0 byte, then Alt/Meta
/// prefixes `ESC` (meta-sends-escape), then UTF-8 passthrough.
fn encode_character(text: &KeyText, mods: &TerminalModifiers) -> Vec<u8> {
    let s = text.as_str();
    if mods.ctrl
        && let Some(byte) = ctrl_letter_byte(s)
    {
        return vec![byte];
    }
    if mods.alt || mods.meta {
        let mut out = vec![0x1b];
        out.extend_from_slice(s.as_bytes());
        return out;
    }
    s.as_bytes().to_vec()
}

fn ctrl_letter_byte(s: &str) -> Option<u8> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let lower = c.to_ascii_lowercase() as u8;
    Some(lower - b'a' + 1)
}

/// Builds `CSI number ; code final_byte`, the xterm form a key sends with
/// Shift, Alt, or Ctrl held, where `code` is the modifier code.
fn modified_sequence(number: u8, code: u8, final_byte: char) -> Vec<u8> {
    format!("\x1b[{number};{code}{final_byte}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(s: &str) -> TerminalKey {
        TerminalKey::Character(KeyText::new(s).expect("test text must be non-empty"))
    }

    fn no_mods() -> TerminalModifiers {
        TerminalModifiers::default()
    }

    fn ctrl() -> TerminalModifiers {
        TerminalModifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn alt() -> TerminalModifiers {
        TerminalModifiers {
            alt: true,
            ..Default::default()
        }
    }

    #[test]
    fn key_text_rejects_empty() {
        assert_eq!(KeyText::new(""), None);
    }

    #[test]
    fn key_text_keeps_non_empty_input() {
        assert_eq!(
            KeyText::new("a").map(|t| t.as_str().to_owned()),
            Some("a".to_owned())
        );
    }

    #[test]
    fn ascii_text_is_passthrough() {
        assert_eq!(
            encode_key(&character("a"), &no_mods(), false, KeypadMode::Numeric),
            b"a".to_vec()
        );
    }

    #[test]
    fn multibyte_text_is_utf8_passthrough() {
        assert_eq!(
            encode_key(&character("あ"), &no_mods(), false, KeypadMode::Numeric),
            "あ".as_bytes().to_vec()
        );
    }

    #[test]
    fn enter_is_carriage_return() {
        assert_eq!(
            encode_key(&TerminalKey::Enter, &no_mods(), false, KeypadMode::Numeric),
            vec![0x0d]
        );
    }

    #[test]
    fn backspace_is_del() {
        assert_eq!(
            encode_key(
                &TerminalKey::Backspace,
                &no_mods(),
                false,
                KeypadMode::Numeric
            ),
            vec![0x7f]
        );
    }

    #[test]
    fn tab_is_horizontal_tab() {
        assert_eq!(
            encode_key(&TerminalKey::Tab, &no_mods(), false, KeypadMode::Numeric),
            vec![0x09]
        );
    }

    /// Asserts that Tab with Shift as the only modifier sends the back
    /// tab sequence `CSI Z`, while Tab with any other modifier added still
    /// sends HT.
    ///
    /// Case: the user presses Shift-Tab to move back through the fields
    /// of a form in a full-screen application.
    #[test]
    fn shift_tab_is_the_back_tab_sequence() {
        let shift = TerminalModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Tab, &shift, false, KeypadMode::Numeric),
            b"\x1b[Z".to_vec()
        );
        let ctrl_shift = TerminalModifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Tab, &ctrl_shift, false, KeypadMode::Numeric),
            vec![0x09]
        );
        for other in [
            TerminalModifiers {
                alt: true,
                shift: true,
                ..Default::default()
            },
            TerminalModifiers {
                meta: true,
                shift: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                encode_key(&TerminalKey::Tab, &other, false, KeypadMode::Numeric),
                vec![0x09],
                "{other:?} leaves Tab as HT"
            );
        }
    }

    #[test]
    fn escape_is_esc() {
        assert_eq!(
            encode_key(&TerminalKey::Escape, &no_mods(), false, KeypadMode::Numeric),
            vec![0x1b]
        );
    }

    /// Asserts that the editing keypad keys send the same sequences
    /// whether or not DECCKM is set.
    ///
    /// Case: a full-screen editor turns on application cursor keys, and
    /// the user presses Insert, Delete, PageUp, or PageDown in it.
    #[test]
    fn editing_keypad_ignores_cursor_mode() {
        for app_cursor in [false, true] {
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::Insert),
                    &no_mods(),
                    app_cursor,
                    KeypadMode::Numeric
                ),
                b"\x1b[2~".to_vec()
            );
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::Delete),
                    &no_mods(),
                    app_cursor,
                    KeypadMode::Numeric
                ),
                b"\x1b[3~".to_vec()
            );
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::PageUp),
                    &no_mods(),
                    app_cursor,
                    KeypadMode::Numeric
                ),
                b"\x1b[5~".to_vec()
            );
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::PageDown),
                    &no_mods(),
                    app_cursor,
                    KeypadMode::Numeric
                ),
                b"\x1b[6~".to_vec()
            );
        }
    }

    #[test]
    fn arrows_use_ss3_in_application_cursor_mode() {
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Up),
                &no_mods(),
                true,
                KeypadMode::Numeric
            ),
            b"\x1bOA".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Down),
                &no_mods(),
                true,
                KeypadMode::Numeric
            ),
            b"\x1bOB".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Right),
                &no_mods(),
                true,
                KeypadMode::Numeric
            ),
            b"\x1bOC".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Left),
                &no_mods(),
                true,
                KeypadMode::Numeric
            ),
            b"\x1bOD".to_vec()
        );
    }

    #[test]
    fn ctrl_letter_collapses_to_c0_byte() {
        assert_eq!(
            encode_key(&character("a"), &ctrl(), false, KeypadMode::Numeric),
            vec![0x01]
        );
        assert_eq!(
            encode_key(&character("c"), &ctrl(), false, KeypadMode::Numeric),
            vec![0x03]
        );
        assert_eq!(
            encode_key(&character("z"), &ctrl(), false, KeypadMode::Numeric),
            vec![0x1a]
        );
    }

    #[test]
    fn ctrl_uppercase_letter_collapses_to_same_byte() {
        assert_eq!(
            encode_key(&character("C"), &ctrl(), false, KeypadMode::Numeric),
            vec![0x03]
        );
    }

    #[test]
    fn ctrl_space_is_unmapped_in_mvp() {
        assert_eq!(
            encode_key(&character(" "), &ctrl(), false, KeypadMode::Numeric),
            b" ".to_vec()
        );
    }

    #[test]
    fn ctrl_digit_is_text_passthrough() {
        assert_eq!(
            encode_key(&character("1"), &ctrl(), false, KeypadMode::Numeric),
            b"1".to_vec()
        );
    }

    #[test]
    fn alt_letter_is_esc_prefixed() {
        assert_eq!(
            encode_key(&character("h"), &alt(), false, KeypadMode::Numeric),
            b"\x1bh".to_vec()
        );
    }

    #[test]
    fn meta_letter_is_esc_prefixed() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&character("x"), &meta, false, KeypadMode::Numeric),
            b"\x1bx".to_vec()
        );
    }

    #[test]
    fn ctrl_takes_priority_over_alt_for_letters() {
        let both = TerminalModifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&character("a"), &both, false, KeypadMode::Numeric),
            vec![0x01]
        );
    }
}
