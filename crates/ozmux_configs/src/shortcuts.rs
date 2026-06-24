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

/// A table of single-chord → tmux-command overrides (`[shortcuts.commands]`).
/// Each chord, when pressed in tmux mode, runs its command instead of tmux's
/// own binding for that key. Named `CommandOverrides` to avoid confusion with
/// Bevy's `Commands`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CommandOverrides(BTreeMap<KeyChord, String>);

impl CommandOverrides {
    /// Yields each `(chord, command)` entry.
    pub fn iter(&self) -> impl Iterator<Item = (&KeyChord, &str)> + '_ {
        self.0.iter().map(|(k, v)| (k, v.as_str()))
    }
}

impl FromIterator<(KeyChord, String)> for CommandOverrides {
    fn from_iter<I: IntoIterator<Item = (KeyChord, String)>>(iter: I) -> Self {
        CommandOverrides(iter.into_iter().collect())
    }
}

impl serde::Serialize for CommandOverrides {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_map(self.0.iter().map(|(k, v)| (k.to_string(), v)))
    }
}

impl<'de> serde::Deserialize<'de> for CommandOverrides {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let raw = BTreeMap::<String, String>::deserialize(de)?;
        let mut map = BTreeMap::new();
        for (key, command) in raw {
            let chord = parse_key_chord(&key).map_err(DeError::custom)?;
            if map.insert(chord, command).is_some() {
                return Err(DeError::custom(format!(
                    "chord {key:?} normalizes to an already-bound override chord"
                )));
            }
        }
        Ok(CommandOverrides(map))
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
    /// Labels (kebab-case built-in action names and/or command text) that share
    /// this chord. Length >= 2.
    pub actions: Vec<String>,
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
    /// Single-chord → tmux-command overrides (`[shortcuts.commands]`). Empty by default.
    pub commands: CommandOverrides,
}

impl Shortcuts {
    /// Detects chord collisions across both the built-in `bindings` and the
    /// `commands` overrides. Built-in entries are labeled by their kebab-case
    /// action name; command entries by their command text. Returns the
    /// colliding chords sorted for deterministic error output.
    pub fn validate_no_conflicts(&self) -> Result<(), Vec<DuplicateChord>> {
        let binding_entries = self
            .bindings
            .iter()
            .filter_map(|(label, bound, _)| bound.as_ref().map(|c| (label.to_string(), c)));
        let command_entries = self
            .commands
            .iter()
            .map(|(chord, cmd)| (cmd.to_string(), chord));
        collect_duplicate_chords(binding_entries.chain(command_entries))
    }
}

/// User-facing shortcut configuration. Each Action gets its own named
/// `Option<KeyChord>` field:
///   - `Some(chord)` = bound to that chord
///   - `None`        = explicitly unbound (via TOML `""`)
///
/// TOML reads the `[shortcuts.bindings]` table; the `kebab-case` serde
/// rename maps each `paste = "Cmd+V"` line to the matching field.
/// `#[serde(default)]` at struct level seeds missing fields from
/// `Bindings::default()`. `deny_unknown_fields` rejects typos at load time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Bindings {
    /// Paste the system clipboard into the active terminal.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub paste: Option<KeyChord>,
    /// Releases keyboard focus from a focused webview back to the terminal.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub release_webview_focus: Option<KeyChord>,
    /// Quits the ozmux application.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub quit: Option<KeyChord>,
    /// Enters copy mode (Alacritty vi mode) on the focused terminal in
    /// `AppMode::Default`.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub enter_copy_mode: Option<KeyChord>,
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
            paste: Some(parse_default_chord("Cmd+V")),
            release_webview_focus: Some(parse_default_chord("Ctrl+Shift+Escape")),
            quit: Some(parse_default_chord("Cmd+Q")),
            enter_copy_mode: Some(parse_default_chord("Cmd+S")),
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
            ("quit", &self.quit, ShortcutAction::Quit),
            (
                "enter-copy-mode",
                &self.enter_copy_mode,
                ShortcutAction::EnterCopyMode,
            ),
            (
                "detach-session",
                &self.detach_session,
                ShortcutAction::DetachSession,
            ),
        ]
        .into_iter()
    }

    #[cfg(test)]
    fn validate_no_conflicts(&self) -> Result<(), Vec<DuplicateChord>> {
        collect_duplicate_chords(
            self.iter()
                .filter_map(|(label, bound, _)| bound.as_ref().map(|c| (label.to_string(), c))),
        )
    }
}

/// Groups `(label, chord)` entries by chord and returns any chord shared by two
/// or more entries, sorted by chord for deterministic output.
fn collect_duplicate_chords<'a>(
    entries: impl Iterator<Item = (String, &'a KeyChord)>,
) -> Result<(), Vec<DuplicateChord>> {
    let mut by_chord: BTreeMap<KeyChord, Vec<String>> = BTreeMap::new();
    for (label, chord) in entries {
        by_chord.entry(chord.clone()).or_default().push(label);
    }
    let dupes: Vec<DuplicateChord> = by_chord
        .into_iter()
        .filter(|(_, labels)| labels.len() >= 2)
        .map(|(chord, actions)| DuplicateChord { chord, actions })
        .collect();
    if dupes.is_empty() { Ok(()) } else { Err(dupes) }
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
    /// Quits the ozmux application.
    Quit,
    /// Detaches from the tmux session and returns to Default single-terminal mode.
    DetachSession,
    /// Enters copy mode (Alacritty vi mode) on the focused terminal in
    /// `AppMode::Default`.
    EnterCopyMode,
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
        assert!(b.quit.is_some());
        assert!(b.detach_session.is_some());
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
        assert!(err[0].actions.iter().any(|a| a == "paste"));
        assert!(err[0].actions.iter().any(|a| a == "release-webview-focus"));
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
        // The kebab-case rename applies.
        let expected = r#"{"bindings":{"paste":{"key":"v","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"release-webview-focus":{"key":"Escape","modifiers":{"ctrl":true,"shift":true,"alt":false,"meta":false}},"quit":{"key":"q","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"enter-copy-mode":{"key":"s","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"detach-session":{"key":"d","modifiers":{"ctrl":true,"shift":true,"alt":false,"meta":false}}},"commands":{}}"#;
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
    fn command_overrides_parse_chord_to_command() {
        let m: CommandOverrides = toml::from_str(r#""Cmd+D" = "split-window -h""#).unwrap();
        let got: Vec<(KeyChord, String)> =
            m.iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
        assert_eq!(
            got,
            vec![(
                parse_key_chord("Cmd+D").unwrap(),
                "split-window -h".to_string()
            )]
        );
    }

    #[test]
    fn command_overrides_reject_malformed_chord() {
        assert!(toml::from_str::<CommandOverrides>(r#""Cmd+Foo" = "x""#).is_err());
    }

    #[test]
    fn command_overrides_reject_normalized_duplicate() {
        // "Cmd+S" and "cmd+s" normalize to the same chord.
        let toml_str = "\"Cmd+S\" = \"a\"\n\"cmd+s\" = \"b\"\n";
        assert!(toml::from_str::<CommandOverrides>(toml_str).is_err());
    }

    #[test]
    fn command_overrides_default_is_empty() {
        assert_eq!(CommandOverrides::default().iter().count(), 0);
    }

    #[test]
    fn shortcuts_default_has_empty_commands() {
        assert_eq!(Shortcuts::default().commands.iter().count(), 0);
    }
}
