//! Pure VT-encoder for `TerminalKeyInput`. Translates a logical key + modifiers
//! into the byte sequence the PTY expects. No I/O, no Bevy types — kept
//! pure so unit tests can cover every branch without an `App`.

/// Subset of keys the terminal input codec understands. Keeps the public
/// surface stable and tells callers exactly which keys are wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalKey {
    /// UTF-8 text (single char or multi-codepoint dead-key composition).
    Character(String),
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

/// Modifier flags carried alongside `TerminalKey`. MVP only reads `ctrl`;
/// `shift` / `alt` / `meta` are reserved for future CSI u / modifyOtherKeys
/// support.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

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
pub(super) fn encode_key(
    key: &TerminalKey,
    mods: &TerminalModifiers,
    app_cursor_keys: bool,
) -> Option<Vec<u8>> {
    if let TerminalKey::Character(s) = key
        && mods.ctrl
        && let Some(byte) = ctrl_letter_byte(s)
    {
        return Some(vec![byte]);
    }
    if (mods.alt || mods.meta)
        && let TerminalKey::Character(s) = key
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
    if let TerminalKey::Character(s) = key {
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
