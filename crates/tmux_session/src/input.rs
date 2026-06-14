//! Pure translation of Bevy keyboard input to tmux `send-keys` commands.
//!
//! Forwarded keys route through tmux's key tables (`send-keys -K`), so tmux's
//! prefix + bindings act. Raw bytes (terminal replies) go to a pane via
//! `send-keys -H`. All construction here is pure + unit-tested; the binary's
//! input plugin is a thin adapter.

use bevy::input::keyboard::Key;

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

/// Maps a Bevy logical key + modifiers to a tmux key-name string for
/// `send-keys -K`, or `None` if the key has no tmux representation OR carries
/// `Super` (which is GUI-only and must never be forwarded).
///
/// NOTE: `Shift` is folded into the glyph for `Key::Character` (so a shifted
/// letter arrives already uppercased) — the `S-` prefix is only emitted for
/// non-character named keys (e.g. `S-Up`). `Super` returns `None`: the caller
/// intercepts GUI chords before this and drops any other `Super` key.
pub fn bevy_key_to_tmux_name(key: &Key, mods: KeyMods) -> Option<String> {
    if mods.super_ {
        return None;
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

/// Builds a batched `send-keys -K -c <client>` command for the given key names
/// (one tmux command per frame). `client` and each name are quoted.
pub fn send_keys_command(client: &str, names: &[String]) -> String {
    let mut cmd = format!("send-keys -K -c {}", quote(client));
    for n in names {
        cmd.push(' ');
        cmd.push_str(&quote(n));
    }
    cmd
}

/// Builds a `send-keys -H -t <pane> <hex>…` command injecting raw bytes into a
/// pane (used for terminal replies). `pane` is the tmux pane id like `%3`.
pub fn send_bytes_command(pane: &str, bytes: &[u8]) -> String {
    let mut cmd = format!("send-keys -H -t {}", quote(pane));
    for b in bytes {
        cmd.push_str(&format!(" {b:02x}"));
    }
    cmd
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
fn quote(arg: &str) -> String {
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
            bevy_key_to_tmux_name(&Key::Character("a".into()), m(false, false, false, false)),
            Some("a".to_string())
        );
    }

    #[test]
    fn ctrl_char_gets_c_prefix() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("c".into()), m(true, false, false, false)),
            Some("C-c".to_string())
        );
    }

    #[test]
    fn shift_is_not_prefixed_for_characters() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("A".into()), m(false, false, true, false)),
            Some("A".to_string())
        );
    }

    #[test]
    fn named_keys_map_to_tmux_names() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Enter, m(false, false, false, false)),
            Some("Enter".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::ArrowUp, m(false, false, false, false)),
            Some("Up".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Backspace, m(false, false, false, false)),
            Some("BSpace".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::PageUp, m(false, false, false, false)),
            Some("PageUp".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Delete, m(false, false, false, false)),
            Some("DC".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::F5, m(false, false, false, false)),
            Some("F5".into())
        );
    }

    #[test]
    fn shift_prefixes_named_keys_only() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::ArrowUp, m(false, false, true, false)),
            Some("S-Up".into())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Tab, m(false, false, true, false)),
            Some("BTab".into())
        );
    }

    #[test]
    fn alt_prefixes_m() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("a".into()), m(false, true, false, false)),
            Some("M-a".to_string())
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::ArrowUp, m(true, true, false, false)),
            Some("C-M-Up".to_string())
        );
    }

    #[test]
    fn super_is_never_forwarded() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("p".into()), m(false, false, false, true)),
            None
        );
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Enter, m(false, false, false, true)),
            None
        );
    }

    #[test]
    fn send_keys_batches_and_quotes() {
        assert_eq!(
            send_keys_command("ozmux", &["a".into(), "C-c".into(), "Up".into()]),
            "send-keys -K -c ozmux a C-c Up"
        );
        assert_eq!(
            send_keys_command("pts 3", &["a".into()]),
            "send-keys -K -c 'pts 3' a"
        );
        assert_eq!(
            send_keys_command("c", &[";".into()]),
            "send-keys -K -c c ';'"
        );
    }

    #[test]
    fn send_bytes_hex_encodes() {
        assert_eq!(
            send_bytes_command("%3", &[0x1b, b'[', b'0', b'n']),
            "send-keys -H -t %3 1b 5b 30 6e"
        );
    }
}
