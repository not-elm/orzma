//! Shortcut domain types: keys, modifiers, chords, bindings, actions.

use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Logical key. v0 covers ASCII characters and a small set of named keys.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug)]
pub enum Key {
    /// Single character key (`Key::Char('b')` for `"b"`).
    Char(char),
    /// `Escape` key.
    Escape,
    /// `Space` key.
    Space,
    /// `Enter` key.
    Enter,
    /// `Tab` key.
    Tab,
    /// `Backspace` key.
    Backspace,
    /// `ArrowUp`.
    ArrowUp,
    /// `ArrowDown`.
    ArrowDown,
    /// `ArrowLeft`.
    ArrowLeft,
    /// `ArrowRight`.
    ArrowRight,
    /// `+` literal key (named to disambiguate from the `+` separator in `"Cmd+Plus"`).
    Plus,
    /// Forward-compatibility variant for unknown logical key names.
    Other(String),
}

impl Key {
    fn from_token(s: &str) -> Self {
        match s {
            "Escape" => Key::Escape,
            "Space" => Key::Space,
            "Enter" => Key::Enter,
            "Tab" => Key::Tab,
            "Backspace" => Key::Backspace,
            "ArrowUp" => Key::ArrowUp,
            "ArrowDown" => Key::ArrowDown,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowRight" => Key::ArrowRight,
            "Plus" => Key::Plus,
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Char(c),
                    _ => Key::Other(other.to_string()),
                }
            }
        }
    }
}

impl serde::Serialize for Key {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            Key::Char(c) => {
                let mut buf = [0u8; 4];
                ser.serialize_str(c.encode_utf8(&mut buf))
            }
            Key::Escape => ser.serialize_str("Escape"),
            Key::Space => ser.serialize_str("Space"),
            Key::Enter => ser.serialize_str("Enter"),
            Key::Tab => ser.serialize_str("Tab"),
            Key::Backspace => ser.serialize_str("Backspace"),
            Key::ArrowUp => ser.serialize_str("ArrowUp"),
            Key::ArrowDown => ser.serialize_str("ArrowDown"),
            Key::ArrowLeft => ser.serialize_str("ArrowLeft"),
            Key::ArrowRight => ser.serialize_str("ArrowRight"),
            Key::Plus => ser.serialize_str("Plus"),
            Key::Other(s) => ser.serialize_str(s),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Key {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Ok(Key::from_token(&s))
    }
}

/// Modifier flags accompanying a `Key`.
#[derive(
    Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Debug, Default,
)]
#[serde(default)]
pub struct Modifiers {
    /// `Ctrl` is held.
    pub ctrl: bool,
    /// `Shift` is held.
    pub shift: bool,
    /// `Alt`/`Option` is held.
    pub alt: bool,
    /// `Meta`/`Command`/`Super` is held.
    pub meta: bool,
}

/// A single keyboard chord (key plus modifier set).
#[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug)]
pub struct KeyChord {
    /// Logical key.
    pub key: Key,
    /// Held modifiers.
    #[serde(default)]
    pub modifiers: Modifiers,
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.meta {
            write!(f, "Cmd+")?;
        }
        if self.modifiers.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.modifiers.alt {
            write!(f, "Alt+")?;
        }
        if self.modifiers.shift {
            write!(f, "Shift+")?;
        }
        match &self.key {
            Key::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
            Key::Escape => write!(f, "Escape"),
            Key::Space => write!(f, "Space"),
            Key::Enter => write!(f, "Enter"),
            Key::Tab => write!(f, "Tab"),
            Key::Backspace => write!(f, "Backspace"),
            Key::ArrowUp => write!(f, "ArrowUp"),
            Key::ArrowDown => write!(f, "ArrowDown"),
            Key::ArrowLeft => write!(f, "ArrowLeft"),
            Key::ArrowRight => write!(f, "ArrowRight"),
            Key::Plus => write!(f, "Plus"),
            Key::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Reason a `parse_key_chord` invocation failed. Surfaced via
/// `D::Error::custom` from serde's `deserialize_with`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyChordParseError {
    /// Consecutive `+` or trailing `+` produced an empty token between separators.
    #[error("empty token in chord string (consecutive '+' or trailing '+')")]
    EmptyToken,
    /// A token that is neither a known modifier nor a known named key.
    #[error("unknown named key: {0:?}")]
    UnknownNamedKey(String),
    /// The same modifier bit was set twice. Catches both literal duplicates
    /// (`"Cmd+Cmd+S"`) and alias collisions (`"Cmd+Meta+S"`, both set `meta`).
    #[error("duplicate modifier {token:?} (normalized to {normalized_bit})")]
    DuplicateModifier {
        /// The offending token as it appeared in the input.
        token: String,
        /// Which `Modifiers` bit was already set.
        normalized_bit: &'static str,
    },
    /// More than one non-modifier token appeared in the chord.
    #[error("multiple key tokens in chord string")]
    MultipleKeyTokens,
}

/// Parses `"Cmd+Shift+S"`-shape strings into a `KeyChord`.
///
/// Modifier names are case-insensitive. Aliases: `Cmd` / `Command` / `Meta` /
/// `Super` all set `meta`; `Alt` / `Opt` / `Option` all set `alt`. ASCII letter
/// keys are normalized to lowercase (Shift is held in `Modifiers`, never in
/// key case). Empty string is NOT accepted here; the field-level
/// `deser_chord_or_unbind` handles the unbind case before calling this.
pub fn parse_key_chord(s: &str) -> Result<KeyChord, KeyChordParseError> {
    if s.is_empty() {
        return Err(KeyChordParseError::EmptyToken);
    }
    let tokens: Vec<&str> = s.split('+').collect();
    if tokens.iter().any(|t| t.is_empty()) {
        return Err(KeyChordParseError::EmptyToken);
    }
    let mut mods = Modifiers::default();
    let mut key: Option<Key> = None;
    for token in tokens {
        if let Some((bit, name)) = parse_modifier_to_bit(token) {
            let already_set = (bit.meta && mods.meta)
                || (bit.ctrl && mods.ctrl)
                || (bit.alt && mods.alt)
                || (bit.shift && mods.shift);
            if already_set {
                return Err(KeyChordParseError::DuplicateModifier {
                    token: token.to_string(),
                    normalized_bit: name,
                });
            }
            mods.meta = mods.meta || bit.meta;
            mods.ctrl = mods.ctrl || bit.ctrl;
            mods.alt = mods.alt || bit.alt;
            mods.shift = mods.shift || bit.shift;
        } else {
            if key.is_some() {
                return Err(KeyChordParseError::MultipleKeyTokens);
            }
            let k = Key::from_token(token);
            if let Key::Other(name) = &k {
                return Err(KeyChordParseError::UnknownNamedKey(name.clone()));
            }
            let k = if let Key::Char(c) = k {
                Key::Char(c.to_ascii_lowercase())
            } else {
                k
            };
            key = Some(k);
        }
    }
    let key = key.ok_or(KeyChordParseError::EmptyToken)?;
    Ok(KeyChord {
        key,
        modifiers: mods,
    })
}

/// serde field-level deserializer for `Option<KeyChord>` that interprets
/// the empty string as `None` (unbind) and any other string as a chord
/// to parse via `parse_key_chord`. Apply with
/// `#[serde(deserialize_with = "deser_chord_or_unbind")]` on every
/// `Option<KeyChord>` field of `Bindings`.
pub fn deser_chord_or_unbind<'de, D>(d: D) -> Result<Option<KeyChord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Ok(None);
    }
    parse_key_chord(&s).map(Some).map_err(DeError::custom)
}

/// One chord-collision entry. Carried inside
/// `OzmuxConfigsError::DuplicateChords` (defined in `error.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateChord {
    /// The chord that has multiple bindings.
    pub chord: KeyChord,
    /// Action labels (kebab-case TOML keys) that share this chord. Length >= 2.
    pub actions: Vec<&'static str>,
}

fn parse_modifier_to_bit(token: &str) -> Option<(Modifiers, &'static str)> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "cmd" | "command" | "meta" | "super" => Some((
            Modifiers {
                meta: true,
                ..Default::default()
            },
            "meta",
        )),
        "ctrl" => Some((
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            "ctrl",
        )),
        "shift" => Some((
            Modifiers {
                shift: true,
                ..Default::default()
            },
            "shift",
        )),
        "alt" | "opt" | "option" => Some((
            Modifiers {
                alt: true,
                ..Default::default()
            },
            "alt",
        )),
        _ => None,
    }
}

/// User-facing shortcut configuration. Wraps the named-field `Bindings`
/// table. Single field — kept as a struct rather than aliased so the
/// existing `/configs/shortcuts` HTTP wire shape stays stable.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Shortcuts {
    /// The named-field binding table. See `Bindings` for the field list and merge semantics.
    pub bindings: Bindings,
}

/// User-facing shortcut configuration. Each Action gets its own named
/// `Option<KeyChord>` field:
///   - `Some(chord)` = bound to that chord
///   - `None`        = explicitly unbound (via TOML `""`)
///
/// TOML reads the `[shortcuts.bindings]` table; the `kebab-case` serde
/// rename maps each `paste = "Cmd+V"` line to the matching field.
/// `#[serde(default)]` at struct level seeds missing fields from
/// `Bindings::default()`. `deny_unknown_fields` rejects typos at load time;
/// deprecated keys (the pane/window/surface set tmux now owns) are accepted
/// and ignored.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Bindings {
    /// Deprecated and ignored: pane lifecycle moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub close_pane: Option<KeyChord>,
    /// Deprecated and ignored: pane focus moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_left: Option<KeyChord>,
    /// Deprecated and ignored: pane focus moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_down: Option<KeyChord>,
    /// Deprecated and ignored: pane focus moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_up: Option<KeyChord>,
    /// Deprecated and ignored: pane focus moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_right: Option<KeyChord>,
    /// Deprecated and ignored: pane splitting moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub split_pane_vertical: Option<KeyChord>,
    /// Deprecated and ignored: pane splitting moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub split_pane_horizontal: Option<KeyChord>,
    /// Deprecated and ignored: pane swapping moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub swap_pane_prev: Option<KeyChord>,
    /// Deprecated and ignored: pane swapping moved to tmux, which owns this
    /// binding now under forward-only key routing. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub swap_pane_next: Option<KeyChord>,
    /// Deprecated and ignored: workspace lifecycle moved to tmux windows, which
    /// own this binding now under forward-only key routing. Accepted so existing
    /// configs carrying it still parse under `deny_unknown_fields`. Remove after
    /// one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub new_workspace: Option<KeyChord>,
    /// Deprecated and ignored: workspace focus moved to tmux windows, which own
    /// this binding now under forward-only key routing. Accepted so existing
    /// configs carrying it still parse under `deny_unknown_fields`. Remove after
    /// one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_workspace_prev: Option<KeyChord>,
    /// Deprecated and ignored: workspace focus moved to tmux windows, which own
    /// this binding now under forward-only key routing. Accepted so existing
    /// configs carrying it still parse under `deny_unknown_fields`. Remove after
    /// one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_workspace_next: Option<KeyChord>,
    /// Paste the system clipboard into the active terminal.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub paste: Option<KeyChord>,
    /// Releases keyboard focus from a focused webview back to the terminal.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub release_webview_focus: Option<KeyChord>,
    /// Deprecated and ignored: renamed to `release_webview_focus`. Accepted so
    /// existing configs carrying it still parse under `deny_unknown_fields`.
    /// Remove after one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub release_inline_focus: Option<KeyChord>,
    /// Opens the tmux session/window picker overlay.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub open_picker: Option<KeyChord>,
    /// Quits the ozmux application.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub quit: Option<KeyChord>,
    /// Deprecated and ignored: this binding was removed when surface/copy
    /// actions were dropped for the tmux backend. Accepted so existing
    /// configs carrying it still parse under `deny_unknown_fields`. Remove
    /// after one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub close_surface: Option<KeyChord>,
    /// Deprecated and ignored: surface creation no longer exists under the
    /// tmux backend. Accepted so existing configs carrying it still parse
    /// under `deny_unknown_fields`. Remove after one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub new_terminal_surface: Option<KeyChord>,
    /// Deprecated and ignored: surface focus cycling was dropped with the
    /// surface model for the tmux backend. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_surface_prev: Option<KeyChord>,
    /// Deprecated and ignored: surface focus cycling was dropped with the
    /// surface model for the tmux backend. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub focus_surface_next: Option<KeyChord>,
    /// Deprecated and ignored: copy mode is now owned by tmux, so this entry
    /// no longer maps to an ozmux action. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub enter_copy_mode: Option<KeyChord>,
    /// Deprecated and ignored: the copy action moved to tmux's own copy mode.
    /// Accepted so existing configs carrying it still parse under
    /// `deny_unknown_fields`. Remove after one release.
    #[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]
    pub copy: Option<KeyChord>,
    /// Detach the current tmux session and switch to Default mode.
    #[serde(deserialize_with = "deser_chord_or_unbind", default)]
    pub detach_session: Option<KeyChord>,
}

fn parse_default_chord(s: &str) -> KeyChord {
    parse_key_chord(s).unwrap_or_else(|e| panic!("invalid default chord {s:?}: {e}"))
}

impl Default for Bindings {
    fn default() -> Self {
        Bindings {
            close_pane: None,
            focus_pane_left: None,
            focus_pane_down: None,
            focus_pane_up: None,
            focus_pane_right: None,
            split_pane_vertical: None,
            split_pane_horizontal: None,
            swap_pane_prev: None,
            swap_pane_next: None,
            new_workspace: None,
            focus_workspace_prev: None,
            focus_workspace_next: None,
            paste: Some(parse_default_chord("Cmd+V")),
            release_webview_focus: Some(parse_default_chord("Ctrl+Shift+Escape")),
            release_inline_focus: None,
            open_picker: Some(parse_default_chord("Cmd+Shift+P")),
            quit: Some(parse_default_chord("Cmd+Q")),
            close_surface: None,
            new_terminal_surface: None,
            focus_surface_prev: None,
            focus_surface_next: None,
            enter_copy_mode: None,
            copy: None,
            detach_session: Some(parse_default_chord("Ctrl+Shift+D")),
        }
    }
}

impl Bindings {
    /// Yields `(action_label, &Option<KeyChord>, Action)` for every
    /// implemented Action. Single source of truth for
    /// `validate_no_conflicts()` and external counters
    /// (e.g., daemon bootstrap binding count).
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (&'static str, &Option<KeyChord>, ShortcutAction)> + '_ {
        [
            ("paste", &self.paste, ShortcutAction::Paste),
            (
                "release-webview-focus",
                &self.release_webview_focus,
                ShortcutAction::ReleaseWebviewFocus,
            ),
            ("open-picker", &self.open_picker, ShortcutAction::OpenPicker),
            ("quit", &self.quit, ShortcutAction::Quit),
            (
                "detach-session",
                &self.detach_session,
                ShortcutAction::DetachSession,
            ),
        ]
        .into_iter()
    }

    /// Detects chord collisions across fields. Returns a `Vec` sorted by chord
    /// (via BTreeMap key order) for deterministic error output. Caller maps
    /// the Vec into `OzmuxConfigsError::DuplicateChords`.
    pub fn validate_no_conflicts(&self) -> Result<(), Vec<DuplicateChord>> {
        let mut by_chord: BTreeMap<KeyChord, Vec<&'static str>> = BTreeMap::new();
        for (label, bound, _action) in self.iter() {
            if let Some(chord) = bound {
                by_chord.entry(chord.clone()).or_default().push(label);
            }
        }
        let dupes: Vec<DuplicateChord> = by_chord
            .into_iter()
            .filter(|(_, labels)| labels.len() >= 2)
            .map(|(chord, actions)| DuplicateChord { chord, actions })
            .collect();
        if dupes.is_empty() { Ok(()) } else { Err(dupes) }
    }
}

/// Shortcut actions reachable under forward-only key routing. tmux owns the
/// pane/window operations now; these are the ozmux-local GUI actions.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ShortcutAction {
    /// Paste the system clipboard into the active terminal.
    Paste,
    /// Releases keyboard focus from a focused webview back to the terminal.
    ReleaseWebviewFocus,
    /// Opens the tmux session/window picker overlay.
    OpenPicker,
    /// Quits the ozmux application.
    Quit,
    /// Detaches from the tmux session and returns to Default single-terminal mode.
    DetachSession,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_parses_single_char() {
        let v: Key = serde_json::from_str("\"b\"").unwrap();
        assert_eq!(v, Key::Char('b'));
    }

    #[test]
    fn key_parses_named_escape() {
        let v: Key = serde_json::from_str("\"Escape\"").unwrap();
        assert_eq!(v, Key::Escape);
    }

    #[test]
    fn key_parses_named_arrow_up_lowercase() {
        let v: Key = serde_json::from_str("\"ArrowUp\"").unwrap();
        assert_eq!(v, Key::ArrowUp);
    }

    #[test]
    fn key_parses_unknown_as_other() {
        let v: Key = serde_json::from_str("\"f12\"").unwrap();
        assert_eq!(v, Key::Other("f12".to_string()));
    }

    #[test]
    fn key_parses_named_plus() {
        let v: Key = serde_json::from_str("\"Plus\"").unwrap();
        assert_eq!(v, Key::Plus);
    }

    #[test]
    fn key_plus_roundtrip() {
        let key = Key::Plus;
        let s = serde_json::to_string(&key).unwrap();
        assert_eq!(s, "\"Plus\"");
        let back: Key = serde_json::from_str(&s).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn key_roundtrip_char() {
        let key = Key::Char('x');
        let s = serde_json::to_string(&key).unwrap();
        assert_eq!(s, "\"x\"");
        let back: Key = serde_json::from_str(&s).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn keychord_display_simple() {
        let c = KeyChord {
            key: Key::Char('s'),
            modifiers: Modifiers {
                meta: true,
                shift: true,
                ctrl: false,
                alt: false,
            },
        };
        assert_eq!(c.to_string(), "Cmd+Shift+S");
    }

    #[test]
    fn keychord_display_named_key() {
        let c = KeyChord {
            key: Key::Escape,
            modifiers: Modifiers::default(),
        };
        assert_eq!(c.to_string(), "Escape");
    }

    #[test]
    fn keychord_display_plus_key() {
        let c = KeyChord {
            key: Key::Plus,
            modifiers: Modifiers {
                meta: true,
                ..Default::default()
            },
        };
        assert_eq!(c.to_string(), "Cmd+Plus");
    }

    #[test]
    fn keychord_display_modifier_order_meta_ctrl_alt_shift_then_key() {
        let c = KeyChord {
            key: Key::Char('a'),
            modifiers: Modifiers {
                meta: true,
                ctrl: true,
                alt: true,
                shift: true,
            },
        };
        assert_eq!(c.to_string(), "Cmd+Ctrl+Alt+Shift+A");
    }

    #[test]
    fn parse_simple_cmd_shift_s() {
        let c = parse_key_chord("Cmd+Shift+S").unwrap();
        assert_eq!(c.key, Key::Char('s'));
        assert!(c.modifiers.meta && c.modifiers.shift);
        assert!(!c.modifiers.ctrl && !c.modifiers.alt);
    }

    #[test]
    fn parse_lowercases_letter() {
        let upper = parse_key_chord("Cmd+S").unwrap();
        let lower = parse_key_chord("Cmd+s").unwrap();
        assert_eq!(upper, lower);
        assert_eq!(upper.key, Key::Char('s'));
    }

    #[test]
    fn parse_modifier_aliases() {
        let cmd = parse_key_chord("Cmd+A").unwrap();
        let command = parse_key_chord("Command+A").unwrap();
        let meta = parse_key_chord("Meta+A").unwrap();
        let super_ = parse_key_chord("Super+A").unwrap();
        assert_eq!(cmd, command);
        assert_eq!(cmd, meta);
        assert_eq!(cmd, super_);
    }

    #[test]
    fn parse_named_keys() {
        assert_eq!(parse_key_chord("Escape").unwrap().key, Key::Escape);
        assert_eq!(parse_key_chord("Cmd+ArrowUp").unwrap().key, Key::ArrowUp);
        assert_eq!(parse_key_chord("Space").unwrap().key, Key::Space);
    }

    #[test]
    fn parse_accepts_plus_as_named_key() {
        let c = parse_key_chord("Cmd+Plus").unwrap();
        assert_eq!(c.key, Key::Plus);
        assert!(c.modifiers.meta);
    }

    #[test]
    fn parse_rejects_unknown_named_key() {
        assert!(parse_key_chord("Cmd+Foo").is_err());
    }

    #[test]
    fn parse_rejects_duplicate_modifier_literal() {
        assert!(parse_key_chord("Cmd+Cmd+S").is_err());
    }

    #[test]
    fn parse_rejects_duplicate_modifier_alias() {
        assert!(parse_key_chord("Cmd+Meta+S").is_err());
    }

    #[test]
    fn parse_rejects_duplicate_modifier_alias_super() {
        assert!(parse_key_chord("Cmd+Super+S").is_err());
    }

    #[test]
    fn parse_rejects_multiple_keys() {
        assert!(parse_key_chord("Cmd+S+T").is_err());
    }

    #[test]
    fn parse_rejects_trailing_plus() {
        assert!(parse_key_chord("Cmd+").is_err());
    }

    #[test]
    fn parse_rejects_consecutive_plus() {
        assert!(parse_key_chord("Cmd++").is_err());
    }

    #[test]
    fn parse_modifier_case_insensitive() {
        assert_eq!(
            parse_key_chord("cmd+s").unwrap(),
            parse_key_chord("CMD+S").unwrap()
        );
    }

    #[test]
    fn deser_chord_or_unbind_handles_empty_string() {
        let json = r#"{"v":""}"#;
        let parsed: OptionWrapper = serde_json::from_str(json).unwrap();
        assert!(parsed.v.is_none());
    }

    #[test]
    fn deser_chord_or_unbind_handles_valid_chord() {
        let json = r#"{"v":"Cmd+S"}"#;
        let parsed: OptionWrapper = serde_json::from_str(json).unwrap();
        let c = parsed.v.unwrap();
        assert_eq!(c.key, Key::Char('s'));
        assert!(c.modifiers.meta);
    }

    #[derive(serde::Deserialize)]
    struct OptionWrapper {
        #[serde(deserialize_with = "deser_chord_or_unbind")]
        v: Option<KeyChord>,
    }

    #[test]
    fn bindings_default_has_active_fields_some() {
        let b = Bindings::default();
        assert!(b.paste.is_some());
        assert!(b.release_webview_focus.is_some());
        assert!(b.open_picker.is_some());
        assert!(b.quit.is_some());
        assert!(b.detach_session.is_some());
    }

    #[test]
    fn bindings_default_deprecated_pane_window_fields_are_none() {
        let b = Bindings::default();
        assert!(b.close_pane.is_none());
        assert!(b.focus_pane_left.is_none());
        assert!(b.focus_pane_down.is_none());
        assert!(b.focus_pane_up.is_none());
        assert!(b.focus_pane_right.is_none());
        assert!(b.split_pane_vertical.is_none());
        assert!(b.split_pane_horizontal.is_none());
        assert!(b.swap_pane_prev.is_none());
        assert!(b.swap_pane_next.is_none());
        assert!(b.new_workspace.is_none());
        assert!(b.focus_workspace_prev.is_none());
        assert!(b.focus_workspace_next.is_none());
    }

    #[test]
    fn validate_no_conflicts_default_ok() {
        let b = Bindings::default();
        assert!(b.validate_no_conflicts().is_ok());
    }

    #[test]
    fn validate_no_conflicts_detects_user_conflict() {
        let b = Bindings {
            paste: Some(parse_key_chord("Ctrl+Shift+Escape").unwrap()),
            ..Default::default()
        };
        let err = b.validate_no_conflicts().unwrap_err();
        assert_eq!(err.len(), 1, "exactly one duplicate-chord entry");
        assert!(err[0].actions.contains(&"paste"));
        assert!(err[0].actions.contains(&"release-webview-focus"));
    }

    #[test]
    fn iter_yields_5_entries() {
        let b = Bindings::default();
        assert_eq!(b.iter().count(), 5);
    }

    #[test]
    fn default_shortcuts_json_snapshot() {
        let json = serde_json::to_string(&Shortcuts::default()).unwrap();
        // The Bindings struct serializes its fields in declaration order.
        // The kebab-case rename applies. Deprecated fields carry
        // `skip_serializing`, so only the active bindings appear here.
        let expected = r#"{"bindings":{"paste":{"key":"v","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"release-webview-focus":{"key":"Escape","modifiers":{"ctrl":true,"shift":true,"alt":false,"meta":false}},"open-picker":{"key":"p","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":true}},"quit":{"key":"q","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"detach-session":{"key":"d","modifiers":{"ctrl":true,"shift":true,"alt":false,"meta":false}}}}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn bindings_default_paste_is_cmd_v() {
        let b = Bindings::default();
        let chord = b.paste.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('v'));
        assert!(chord.modifiers.meta);
        assert!(!chord.modifiers.ctrl && !chord.modifiers.shift && !chord.modifiers.alt);
    }

    #[test]
    fn bindings_default_open_picker_is_cmd_shift_p() {
        let b = Bindings::default();
        let chord = b.open_picker.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('p'));
        assert!(chord.modifiers.meta && chord.modifiers.shift);
        assert!(!chord.modifiers.ctrl && !chord.modifiers.alt);
    }

    #[test]
    fn bindings_default_quit_is_cmd_q() {
        let b = Bindings::default();
        let chord = b.quit.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('q'));
        assert!(chord.modifiers.meta);
        assert!(!chord.modifiers.ctrl && !chord.modifiers.shift && !chord.modifiers.alt);
    }

    #[test]
    fn iter_binds_cmd_v_to_paste() {
        let b = Bindings::default();
        let chord = parse_key_chord("Cmd+V").unwrap();
        let action = b
            .iter()
            .find_map(|(_, bound, action)| (bound.as_ref() == Some(&chord)).then_some(action))
            .expect("Cmd+V must resolve");
        assert!(matches!(action, ShortcutAction::Paste));
    }

    #[test]
    fn deprecated_surface_copy_bindings_are_accepted_and_ignored() {
        // Configs carrying the removed surface/copy-mode binding keys must
        // still parse (back-compat) and not appear in the active binding set.
        let toml = "\
[bindings]
close-surface = \"Cmd+Shift+F\"
new-terminal-surface = \"Cmd+Shift+T\"
focus-surface-prev = \"Cmd+Shift+G\"
focus-surface-next = \"Cmd+Shift+B\"
enter-copy-mode = \"Cmd+U\"
copy = \"Cmd+C\"
";
        let parsed: Shortcuts = toml::from_str(toml).expect("deprecated keys must still parse");
        assert_eq!(
            parsed.bindings.iter().count(),
            5,
            "ignored keys must not enter the active set"
        );
    }

    #[test]
    fn deprecated_release_inline_focus_key_still_parses_and_is_ignored() {
        // Old configs carrying the renamed key must still parse under
        // deny_unknown_fields, and must not appear in the active binding set.
        let toml = "[bindings]\nrelease-inline-focus = \"Cmd+E\"\n";
        let parsed: Shortcuts = toml::from_str(toml).expect("deprecated key must still parse");
        assert!(
            parsed
                .bindings
                .iter()
                .all(|(label, _, _)| label != "release-inline-focus"),
            "deprecated key must not enter the active set"
        );
        // The new field falls back to its default chord.
        assert!(parsed.bindings.release_webview_focus.is_some());
    }

    #[test]
    fn deprecated_pane_window_bindings_are_accepted_and_ignored() {
        let toml = "\
[bindings]
split-pane-vertical = \"Cmd+I\"
focus-pane-left = \"Cmd+H\"
new-workspace = \"Cmd+R\"
swap-pane-next = \"Cmd+N\"
";
        let parsed: Shortcuts = toml::from_str(toml).expect("deprecated keys must still parse");
        assert!(
            parsed.bindings.iter().all(|(label, _, _)| !matches!(
                label,
                "split-pane-vertical" | "focus-pane-left" | "new-workspace" | "swap-pane-next"
            )),
            "ignored keys must not enter the active set",
        );
    }
}
