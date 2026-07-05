//! Pure VT-encoder for `TerminalKeyInput`. Translates a logical key + modifiers
//! into the byte sequence the PTY expects. No I/O, no Bevy types — kept
//! pure so unit tests can cover every branch without an `App`.

use crate::events::{TerminalKey, TerminalModifiers};

/// Translates a logical key + modifiers into VT-escape bytes.
///
/// Priority order:
/// 1. Ctrl + ASCII letter → `0x01..=0x1A` (1 byte)
/// 2. Alt/Meta + `Text(s)` → `ESC` + `s.as_bytes()` (meta-sends-escape)
/// 3. Arrow keys → `ESC [ A/B/C/D` (normal) or `ESC O A/B/C/D` (app-cursor)
/// 4. Special key table (Enter, Backspace, Tab, Escape, Delete, Home, End,
///    PageUp, PageDown)
/// 5. `Text(s)` fallback → `s.as_bytes()` (UTF-8 passthrough). Empty → `None`.
///
/// Returns `None` if the key/modifier combination produces no PTY output
/// (e.g. empty `Text`, unmapped combination).
pub(crate) fn encode_key(
    key: &TerminalKey,
    mods: &TerminalModifiers,
    app_cursor_keys: bool,
) -> Option<Vec<u8>> {
    if let TerminalKey::Text(s) = key
        && mods.ctrl
        && let Some(byte) = ctrl_letter_byte(s)
    {
        return Some(vec![byte]);
    }
    // NOTE: meta-sends-escape — Alt/Meta on a Text key emits ESC + the key
    // bytes, which crossterm decodes back as an Alt-modified key. Placed after
    // the Ctrl-letter branch so Ctrl keeps priority for letters.
    if (mods.alt || mods.meta)
        && let TerminalKey::Text(s) = key
        && !s.is_empty()
    {
        let mut out = vec![0x1b];
        out.extend_from_slice(s.as_bytes());
        return Some(out);
    }
    if let Some(bytes) = arrow_bytes(key, app_cursor_keys) {
        return Some(bytes);
    }
    if let Some(bytes) = special_bytes(key) {
        return Some(bytes);
    }
    if let TerminalKey::Text(s) = key {
        if s.is_empty() {
            return None;
        }
        return Some(s.as_bytes().to_vec());
    }
    None
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

fn arrow_bytes(key: &TerminalKey, app_cursor_keys: bool) -> Option<Vec<u8>> {
    let suffix = match key {
        TerminalKey::ArrowUp => b'A',
        TerminalKey::ArrowDown => b'B',
        TerminalKey::ArrowRight => b'C',
        TerminalKey::ArrowLeft => b'D',
        _ => return None,
    };
    let prefix = if app_cursor_keys { b'O' } else { b'[' };
    Some(vec![0x1b, prefix, suffix])
}

fn special_bytes(key: &TerminalKey) -> Option<Vec<u8>> {
    Some(match key {
        TerminalKey::Enter => vec![0x0d],
        TerminalKey::Backspace => vec![0x7f],
        TerminalKey::Tab => vec![0x09],
        TerminalKey::Escape => vec![0x1b],
        TerminalKey::Delete => b"\x1b[3~".to_vec(),
        TerminalKey::Home => b"\x1b[H".to_vec(),
        TerminalKey::End => b"\x1b[F".to_vec(),
        TerminalKey::PageUp => b"\x1b[5~".to_vec(),
        TerminalKey::PageDown => b"\x1b[6~".to_vec(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn printable_ascii_passes_through_as_utf8() {
        assert_eq!(
            encode_key(&TerminalKey::Text("a".into()), &no_mods(), false),
            Some(b"a".to_vec())
        );
    }

    #[test]
    fn multibyte_utf8_passes_through() {
        let bytes = encode_key(&TerminalKey::Text("あ".into()), &no_mods(), false).unwrap();
        assert_eq!(bytes, "あ".as_bytes());
    }

    #[test]
    fn empty_text_returns_none() {
        assert_eq!(
            encode_key(&TerminalKey::Text(String::new()), &no_mods(), false),
            None
        );
    }

    #[test]
    fn enter_is_cr() {
        assert_eq!(
            encode_key(&TerminalKey::Enter, &no_mods(), false),
            Some(vec![0x0d])
        );
    }

    #[test]
    fn backspace_is_del() {
        assert_eq!(
            encode_key(&TerminalKey::Backspace, &no_mods(), false),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn tab_is_ht() {
        assert_eq!(
            encode_key(&TerminalKey::Tab, &no_mods(), false),
            Some(vec![0x09])
        );
    }

    #[test]
    fn escape_is_esc() {
        assert_eq!(
            encode_key(&TerminalKey::Escape, &no_mods(), false),
            Some(vec![0x1b])
        );
    }

    #[test]
    fn delete_is_csi_3_tilde() {
        assert_eq!(
            encode_key(&TerminalKey::Delete, &no_mods(), false),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn home_is_csi_h() {
        assert_eq!(
            encode_key(&TerminalKey::Home, &no_mods(), false),
            Some(b"\x1b[H".to_vec())
        );
    }

    #[test]
    fn end_is_csi_f() {
        assert_eq!(
            encode_key(&TerminalKey::End, &no_mods(), false),
            Some(b"\x1b[F".to_vec())
        );
    }

    #[test]
    fn page_up_is_csi_5_tilde() {
        assert_eq!(
            encode_key(&TerminalKey::PageUp, &no_mods(), false),
            Some(b"\x1b[5~".to_vec())
        );
    }

    #[test]
    fn page_down_is_csi_6_tilde() {
        assert_eq!(
            encode_key(&TerminalKey::PageDown, &no_mods(), false),
            Some(b"\x1b[6~".to_vec())
        );
    }

    #[test]
    fn ctrl_a_is_soh() {
        assert_eq!(
            encode_key(&TerminalKey::Text("a".into()), &ctrl(), false),
            Some(vec![0x01])
        );
    }

    #[test]
    fn ctrl_c_is_etx() {
        assert_eq!(
            encode_key(&TerminalKey::Text("c".into()), &ctrl(), false),
            Some(vec![0x03])
        );
    }

    #[test]
    fn ctrl_z_is_sub() {
        assert_eq!(
            encode_key(&TerminalKey::Text("z".into()), &ctrl(), false),
            Some(vec![0x1a])
        );
    }

    #[test]
    fn ctrl_uppercase_letter_normalizes_to_same_byte() {
        assert_eq!(
            encode_key(&TerminalKey::Text("C".into()), &ctrl(), false),
            Some(vec![0x03])
        );
    }

    #[test]
    fn arrow_normal_mode_uses_csi_prefix() {
        assert_eq!(
            encode_key(&TerminalKey::ArrowUp, &no_mods(), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowDown, &no_mods(), false),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowRight, &no_mods(), false),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowLeft, &no_mods(), false),
            Some(b"\x1b[D".to_vec())
        );
    }

    #[test]
    fn arrow_app_cursor_mode_uses_ss3_prefix() {
        assert_eq!(
            encode_key(&TerminalKey::ArrowUp, &no_mods(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowDown, &no_mods(), true),
            Some(b"\x1bOB".to_vec())
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowRight, &no_mods(), true),
            Some(b"\x1bOC".to_vec())
        );
        assert_eq!(
            encode_key(&TerminalKey::ArrowLeft, &no_mods(), true),
            Some(b"\x1bOD".to_vec())
        );
    }

    #[test]
    fn ctrl_space_is_unmapped_in_mvp() {
        assert_eq!(
            encode_key(&TerminalKey::Text(" ".into()), &ctrl(), false),
            Some(b" ".to_vec())
        );
    }

    #[test]
    fn ctrl_digit_is_text_passthrough() {
        assert_eq!(
            encode_key(&TerminalKey::Text("1".into()), &ctrl(), false),
            Some(b"1".to_vec())
        );
    }

    #[test]
    fn alt_letter_is_esc_prefixed() {
        assert_eq!(
            encode_key(&TerminalKey::Text("h".into()), &alt(), false),
            Some(b"\x1bh".to_vec())
        );
    }

    #[test]
    fn meta_letter_is_esc_prefixed() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Text("x".into()), &meta, false),
            Some(b"\x1bx".to_vec())
        );
    }

    #[test]
    fn ctrl_takes_priority_over_alt_for_letters() {
        let ctrl = TerminalModifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Text("a".into()), &ctrl, false),
            Some(vec![0x01])
        );
    }
}
