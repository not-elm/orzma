//! Pure translation of Bevy keyboard input to tmux `send-keys` commands.
//!
//! Forwarded keys go straight to the active pane (`send-keys -t <pane> --`),
//! which tmux's pane-input encoder translates (respecting the pane's
//! application-cursor mode). `-K` is deliberately NOT used: under `tmux -CC` it
//! mis-encodes named keys (e.g. `Up` arrives as a literal `n`), and routing
//! through the client key tables is not needed since ozmux owns its own chords.
//! Raw bytes (clipboard paste) go to a pane via `send-keys -H`. All
//! construction here is pure + unit-tested; the binary's input plugin is a thin
//! adapter.

use bevy::input::keyboard::{Key, KeyCode};

/// Active keyboard modifiers for a key event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyMods {
    /// Control.
    pub ctrl: bool,
    /// Alt / Option (tmux `M-`).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Command / Super — tmux has NO equivalent; GUI-only (see `bevy_key_to_tmux_name`).
    pub super_: bool,
}

/// Maps a Bevy logical key + physical key code + modifiers to a tmux key-name
/// string (used to forward the key pane-direct and to match it against tmux
/// bindings), or `None` if the key has no tmux representation OR carries `Super`
/// (which is GUI-only and must never be forwarded).
///
/// NOTE: `Shift` is folded into the glyph for `Key::Character` (so a shifted
/// letter arrives already uppercased) — the `S-` prefix is only emitted for
/// non-character named keys (e.g. `S-Up`). `Super` returns `None`: the caller
/// intercepts GUI chords before this and drops any other `Super` key.
///
/// NOTE: when `Alt` is held, the base key is taken from the physical `code`, not
/// the logical glyph. macOS composes Option-modified keys (Option+p → "π"),
/// which tmux cannot map to its `M-p` bindings (it types `<ffffffff>`); tmux
/// `M-` bindings are defined on the base key, so `Alt` acts as Meta here. `Shift`
/// is folded into the base by uppercasing it (Alt+Shift+p → `M-P`), matching the
/// `Key::Character` convention. Letter, digit, and ASCII-punctuation keys have a
/// physical base (so Alt+. → `M-.` despite macOS composing Option+. into `≥`);
/// other Alt-modified keys fall through to the logical glyph below. Punctuation
/// has no ASCII uppercase, so the Shift fold leaves it unchanged (Alt+Shift+. →
/// `M-.`).
pub fn bevy_key_to_tmux_name(key: &Key, code: KeyCode, mods: KeyMods) -> Option<String> {
    if mods.super_ {
        return None;
    }
    if mods.alt
        && let Some(base) = physical_base_char(code)
    {
        let base = if mods.shift {
            base.to_ascii_uppercase()
        } else {
            base
        };
        return Some(prefix(&mods, false, &base.to_string()));
    }
    let base = match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            c.to_string()
        }
        Key::Enter => "Enter".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Tab if mods.shift => return Some(prefix(&mods, false, "BTab")),
        Key::Tab => "Tab".to_string(),
        Key::Backspace => "BSpace".to_string(),
        Key::Space => "Space".to_string(),
        Key::ArrowUp => "Up".to_string(),
        Key::ArrowDown => "Down".to_string(),
        Key::ArrowLeft => "Left".to_string(),
        Key::ArrowRight => "Right".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "PageUp".to_string(),
        Key::PageDown => "PageDown".to_string(),
        Key::Insert => "IC".to_string(),
        Key::Delete => "DC".to_string(),
        Key::F1 => "F1".to_string(),
        Key::F2 => "F2".to_string(),
        Key::F3 => "F3".to_string(),
        Key::F4 => "F4".to_string(),
        Key::F5 => "F5".to_string(),
        Key::F6 => "F6".to_string(),
        Key::F7 => "F7".to_string(),
        Key::F8 => "F8".to_string(),
        Key::F9 => "F9".to_string(),
        Key::F10 => "F10".to_string(),
        Key::F11 => "F11".to_string(),
        Key::F12 => "F12".to_string(),
        _ => return None,
    };
    let shift_prefix = !matches!(key, Key::Character(_));
    Some(prefix(&mods, shift_prefix, &base))
}

/// Returns the base character a letter/digit/ASCII-punctuation physical key
/// produces, ignoring any layout composition (e.g. macOS Option). `None` for
/// non-character keys.
fn physical_base_char(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Backquote => '`',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

/// Prefixes `C-`/`M-`/`S-` modifier tokens onto a tmux key name.
fn prefix(mods: &KeyMods, shift: bool, base: &str) -> String {
    let mut out = String::new();
    if mods.ctrl {
        out.push_str("C-");
    }
    if mods.alt {
        out.push_str("M-");
    }
    if shift && mods.shift {
        out.push_str("S-");
    }
    out.push_str(base);
    out
}

/// Quotes a tmux command argument: wraps in single quotes if it contains
/// whitespace or shell/tmux metacharacters, escaping embedded single quotes.
pub(crate) fn quote(arg: &str) -> String {
    let needs = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || "\"'\\$;|&<>(){}[]*?#`".contains(c));
    if !needs {
        return arg.to_string();
    }
    let escaped = arg.replace('\'', r"'\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ctrl: bool, alt: bool, shift: bool, super_: bool) -> KeyMods {
        KeyMods {
            ctrl,
            alt,
            shift,
            super_,
        }
    }

    #[test]
    fn plain_char_maps_to_itself() {
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("a".into()),
                KeyCode::KeyA,
                m(false, false, false, false)
            ),
            Some("a".to_string())
        );
    }

    #[test]
    fn ctrl_char_gets_c_prefix() {
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("c".into()),
                KeyCode::KeyC,
                m(true, false, false, false)
            ),
            Some("C-c".to_string())
        );
    }

    #[test]
    fn shift_is_not_prefixed_for_characters() {
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("A".into()),
                KeyCode::KeyA,
                m(false, false, true, false)
            ),
            Some("A".to_string())
        );
    }

    #[test]
    fn alt_letter_uses_physical_key_not_composed_glyph() {
        // macOS Option+p yields logical "π"; tmux needs M-p (the base key) to
        // match its `M-p` binding — otherwise it types <ffffffff>.
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("π".into()),
                KeyCode::KeyP,
                m(false, true, false, false)
            ),
            Some("M-p".to_string())
        );
        // Alt+i likewise resolves to the base key regardless of composition.
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("i".into()),
                KeyCode::KeyI,
                m(false, true, false, false)
            ),
            Some("M-i".to_string())
        );
    }

    #[test]
    fn alt_punctuation_uses_physical_key_not_composed_glyph() {
        // macOS Option+. yields logical "≥" (Option+, → "≤"); tmux needs M-.
        // (the base key) to match its `M-.` binding — otherwise it types
        // <ffffffff>. The base comes from the physical key, layout composition
        // notwithstanding.
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("≥".into()),
                KeyCode::Period,
                m(false, true, false, false)
            ),
            Some("M-.".to_string())
        );
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("≤".into()),
                KeyCode::Comma,
                m(false, true, false, false)
            ),
            Some("M-,".to_string())
        );
    }

    #[test]
    fn alt_shift_letter_folds_shift_into_uppercase_base() {
        // Alt+Shift+P must reach tmux as M-P, not M-p; the composed logical
        // glyph is irrelevant — the physical key + Shift decide the base.
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("π".into()),
                KeyCode::KeyP,
                m(false, true, true, false)
            ),
            Some("M-P".to_string())
        );
    }

    #[test]
    fn named_keys_map_to_tmux_names() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Enter, KeyCode::Enter, m(false, false, false, false)),
            Some("Enter".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::ArrowUp,
                KeyCode::ArrowUp,
                m(false, false, false, false)
            ),
            Some("Up".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Backspace,
                KeyCode::Backspace,
                m(false, false, false, false)
            ),
            Some("BSpace".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::PageUp, KeyCode::PageUp, m(false, false, false, false)),
            Some("PageUp".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Delete, KeyCode::Delete, m(false, false, false, false)),
            Some("DC".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::F5, KeyCode::F5, m(false, false, false, false)),
            Some("F5".into())
        );
    }

    #[test]
    fn shift_prefixes_named_keys_only() {
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::ArrowUp,
                KeyCode::ArrowUp,
                m(false, false, true, false)
            ),
            Some("S-Up".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Tab, KeyCode::Tab, m(false, false, true, false)),
            Some("BTab".into())
        );
    }

    #[test]
    fn alt_prefixes_m() {
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("a".into()),
                KeyCode::KeyA,
                m(false, true, false, false)
            ),
            Some("M-a".to_string())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::ArrowUp, KeyCode::ArrowUp, m(true, true, false, false)),
            Some("C-M-Up".to_string())
        );
    }

    #[test]
    fn super_is_never_forwarded() {
        assert_eq!(
            bevy_key_to_tmux_name(
                &Key::Character("p".into()),
                KeyCode::KeyP,
                m(false, false, false, true)
            ),
            None
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Enter, KeyCode::Enter, m(false, false, false, true)),
            None
        );
    }
}
