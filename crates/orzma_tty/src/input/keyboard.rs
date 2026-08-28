//! Pure VT-encoder for key input. Translates a logical key + modifiers
//! into the byte sequence the PTY expects. No I/O, no Bevy types — kept
//! pure so unit tests can cover every branch without an `App`.

mod keypad;

pub use keypad::KeypadKey;

/// Non-empty UTF-8 text carried by [`TerminalKey::Character`].
///
/// The non-empty invariant is what makes `encode_key` total: empty text has
/// no PTY representation, so it is rejected once at construction instead of
/// being reported on every encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyText(String);

impl KeyText {
    /// Wraps `text`, returning `None` when it is empty — there is nothing to
    /// send to the PTY in that case.
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.is_empty()).then_some(Self(text))
    }

    /// Returns the wrapped text, which is never empty.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Subset of keys the terminal input codec understands. Keeps the public
/// surface stable and tells callers exactly which keys are wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalKey {
    /// UTF-8 text (single char or multi-codepoint composition).
    Character(KeyText),
    Enter,
    Backspace,
    Tab,
    Escape,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Modifier flags carried alongside `TerminalKey`.
/// `ctrl` / `alt` / `meta` affect `Character` encoding; `shift` is reserved for future CSI u /
/// modifyOtherKeys support.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Translates a logical key + modifiers into the bytes the PTY expects.
///
/// Total — every `TerminalKey` has a representation, because `Character`
/// cannot carry an empty string.
///
/// The cursor keys — arrows plus Home and End, which xterm also classifies as
/// cursor keys — honour `app_cursor_keys` (DECCKM): `ESC [ A/B/C/D/H/F` in
/// normal mode, `ESC O A/B/C/D/H/F` in application mode. The VT220 editing
/// keypad (Delete, PageUp, PageDown) is unaffected by DECCKM and maps to fixed
/// sequences; `Character` is encoded by `encode_character`.
pub(super) fn encode_key(
    key: &TerminalKey,
    mods: &TerminalModifiers,
    app_cursor_keys: bool,
) -> Vec<u8> {
    match key {
        TerminalKey::Character(text) => encode_character(text, mods),
        TerminalKey::ArrowUp => cursor_key_bytes(b'A', app_cursor_keys),
        TerminalKey::ArrowDown => cursor_key_bytes(b'B', app_cursor_keys),
        TerminalKey::ArrowRight => cursor_key_bytes(b'C', app_cursor_keys),
        TerminalKey::ArrowLeft => cursor_key_bytes(b'D', app_cursor_keys),
        TerminalKey::Home => cursor_key_bytes(b'H', app_cursor_keys),
        TerminalKey::End => cursor_key_bytes(b'F', app_cursor_keys),
        TerminalKey::Enter => vec![0x0d],
        TerminalKey::Backspace => vec![0x7f],
        TerminalKey::Tab => vec![0x09],
        TerminalKey::Escape => vec![0x1b],
        TerminalKey::Delete => b"\x1b[3~".to_vec(),
        TerminalKey::PageUp => b"\x1b[5~".to_vec(),
        TerminalKey::PageDown => b"\x1b[6~".to_vec(),
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

fn cursor_key_bytes(suffix: u8, app_cursor_keys: bool) -> Vec<u8> {
    let prefix = if app_cursor_keys { b'O' } else { b'[' };
    vec![0x1b, prefix, suffix]
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
            encode_key(&character("a"), &no_mods(), false),
            b"a".to_vec()
        );
    }

    #[test]
    fn multibyte_text_is_utf8_passthrough() {
        assert_eq!(
            encode_key(&character("あ"), &no_mods(), false),
            "あ".as_bytes().to_vec()
        );
    }

    #[test]
    fn enter_is_carriage_return() {
        assert_eq!(
            encode_key(&TerminalKey::Enter, &no_mods(), false),
            vec![0x0d]
        );
    }

    #[test]
    fn backspace_is_del() {
        assert_eq!(
            encode_key(&TerminalKey::Backspace, &no_mods(), false),
            vec![0x7f]
        );
    }

    #[test]
    fn tab_is_horizontal_tab() {
        assert_eq!(encode_key(&TerminalKey::Tab, &no_mods(), false), vec![0x09]);
    }

    #[test]
    fn escape_is_esc() {
        assert_eq!(
            encode_key(&TerminalKey::Escape, &no_mods(), false),
            vec![0x1b]
        );
    }

    #[test]
    fn vt220_style_keys_use_tilde_sequences() {
        assert_eq!(
            encode_key(&TerminalKey::Delete, &no_mods(), false),
            b"\x1b[3~".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::PageUp, &no_mods(), false),
            b"\x1b[5~".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::PageDown, &no_mods(), false),
            b"\x1b[6~".to_vec()
        );
    }

    #[test]
    fn home_and_end_use_csi_in_normal_cursor_mode() {
        assert_eq!(
            encode_key(&TerminalKey::Home, &no_mods(), false),
            b"\x1b[H".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::End, &no_mods(), false),
            b"\x1b[F".to_vec()
        );
    }

    #[test]
    fn home_and_end_use_ss3_in_application_cursor_mode() {
        assert_eq!(
            encode_key(&TerminalKey::Home, &no_mods(), true),
            b"\x1bOH".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::End, &no_mods(), true),
            b"\x1bOF".to_vec()
        );
    }

    #[test]
    fn editing_keypad_ignores_cursor_mode() {
        for app_cursor in [false, true] {
            assert_eq!(
                encode_key(&TerminalKey::Delete, &no_mods(), app_cursor),
                b"\x1b[3~".to_vec()
            );
            assert_eq!(
                encode_key(&TerminalKey::PageUp, &no_mods(), app_cursor),
                b"\x1b[5~".to_vec()
            );
            assert_eq!(
                encode_key(&TerminalKey::PageDown, &no_mods(), app_cursor),
                b"\x1b[6~".to_vec()
            );
        }
    }

    #[test]
    fn arrows_use_csi_in_normal_cursor_mode() {
        assert_eq!(
            encode_key(&TerminalKey::ArrowUp, &no_mods(), false),
            b"\x1b[A".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowDown, &no_mods(), false),
            b"\x1b[B".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowRight, &no_mods(), false),
            b"\x1b[C".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowLeft, &no_mods(), false),
            b"\x1b[D".to_vec()
        );
    }

    #[test]
    fn arrows_use_ss3_in_application_cursor_mode() {
        assert_eq!(
            encode_key(&TerminalKey::ArrowUp, &no_mods(), true),
            b"\x1bOA".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowDown, &no_mods(), true),
            b"\x1bOB".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowRight, &no_mods(), true),
            b"\x1bOC".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowLeft, &no_mods(), true),
            b"\x1bOD".to_vec()
        );
    }

    #[test]
    fn ctrl_letter_collapses_to_c0_byte() {
        assert_eq!(encode_key(&character("a"), &ctrl(), false), vec![0x01]);
        assert_eq!(encode_key(&character("c"), &ctrl(), false), vec![0x03]);
        assert_eq!(encode_key(&character("z"), &ctrl(), false), vec![0x1a]);
    }

    #[test]
    fn ctrl_uppercase_letter_collapses_to_same_byte() {
        assert_eq!(encode_key(&character("C"), &ctrl(), false), vec![0x03]);
    }

    #[test]
    fn ctrl_space_is_unmapped_in_mvp() {
        assert_eq!(encode_key(&character(" "), &ctrl(), false), b" ".to_vec());
    }

    #[test]
    fn ctrl_digit_is_text_passthrough() {
        assert_eq!(encode_key(&character("1"), &ctrl(), false), b"1".to_vec());
    }

    #[test]
    fn alt_letter_is_esc_prefixed() {
        assert_eq!(
            encode_key(&character("h"), &alt(), false),
            b"\x1bh".to_vec()
        );
    }

    #[test]
    fn meta_letter_is_esc_prefixed() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(encode_key(&character("x"), &meta, false), b"\x1bx".to_vec());
    }

    #[test]
    fn ctrl_takes_priority_over_alt_for_letters() {
        let both = TerminalModifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(encode_key(&character("a"), &both, false), vec![0x01]);
    }
}
