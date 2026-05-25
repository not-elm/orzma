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
#[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug, Default)]
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
/// rename maps each `close-pane = "Cmd+Shift+D"` line to the matching
/// field. `#[serde(default)]` at struct level seeds missing fields from
/// `Bindings::default()` (the 17 defaults). `deny_unknown_fields` rejects
/// typos and unimplemented-action keys at load time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Bindings {
    /// Close the active pane.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub close_pane: Option<KeyChord>,
    /// Move pane focus left.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_left: Option<KeyChord>,
    /// Move pane focus down.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_down: Option<KeyChord>,
    /// Move pane focus up.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_up: Option<KeyChord>,
    /// Move pane focus right.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_pane_right: Option<KeyChord>,
    /// Split the active pane vertically.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub split_pane_vertical: Option<KeyChord>,
    /// Split the active pane horizontally.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub split_pane_horizontal: Option<KeyChord>,
    /// Swap the active pane with the previous sibling.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub swap_pane_prev: Option<KeyChord>,
    /// Swap the active pane with the next sibling.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub swap_pane_next: Option<KeyChord>,
    /// Close the active activity.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub close_activity: Option<KeyChord>,
    /// Spawn a new terminal activity in the active pane.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub new_terminal_activity: Option<KeyChord>,
    /// Cycle activity focus to the previous one.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_activity_prev: Option<KeyChord>,
    /// Cycle activity focus to the next one.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_activity_next: Option<KeyChord>,
    /// Enter tmux-style copy mode on the active terminal activity.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub enter_copy_mode: Option<KeyChord>,
    /// Create a new window in the active session.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub new_window: Option<KeyChord>,
    /// Cycle window focus to the previous one.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_window_prev: Option<KeyChord>,
    /// Cycle window focus to the next one.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub focus_window_next: Option<KeyChord>,
}

fn parse_default_chord(s: &str) -> KeyChord {
    parse_key_chord(s).unwrap_or_else(|e| panic!("invalid default chord {s:?}: {e}"))
}

impl Default for Bindings {
    fn default() -> Self {
        Bindings {
            close_pane: Some(parse_default_chord("Cmd+Shift+D")),
            focus_pane_left: Some(parse_default_chord("Cmd+H")),
            focus_pane_down: Some(parse_default_chord("Cmd+J")),
            focus_pane_up: Some(parse_default_chord("Cmd+K")),
            focus_pane_right: Some(parse_default_chord("Cmd+L")),
            split_pane_vertical: Some(parse_default_chord("Cmd+I")),
            split_pane_horizontal: Some(parse_default_chord("Cmd+O")),
            swap_pane_prev: Some(parse_default_chord("Cmd+B")),
            swap_pane_next: Some(parse_default_chord("Cmd+N")),
            close_activity: Some(parse_default_chord("Cmd+Shift+F")),
            new_terminal_activity: Some(parse_default_chord("Cmd+T")),
            focus_activity_prev: Some(parse_default_chord("Cmd+[")),
            focus_activity_next: Some(parse_default_chord("Cmd+]")),
            enter_copy_mode: Some(parse_default_chord("Cmd+U")),
            new_window: Some(parse_default_chord("Cmd+R")),
            focus_window_prev: Some(parse_default_chord("Cmd+Shift+[")),
            focus_window_next: Some(parse_default_chord("Cmd+Shift+]")),
        }
    }
}

impl Bindings {
    /// Yields `(action_label, &Option<KeyChord>, Action)` for every
    /// implemented Action. Single source of truth for `lookup()`,
    /// `validate_no_conflicts()`, and external counters
    /// (e.g., daemon bootstrap binding count).
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &Option<KeyChord>, Action)> + '_ {
        [
            ("close-pane", &self.close_pane, Action::ClosePane),
            (
                "focus-pane-left",
                &self.focus_pane_left,
                Action::FocusPane {
                    direction: Direction::Left,
                },
            ),
            (
                "focus-pane-down",
                &self.focus_pane_down,
                Action::FocusPane {
                    direction: Direction::Down,
                },
            ),
            (
                "focus-pane-up",
                &self.focus_pane_up,
                Action::FocusPane {
                    direction: Direction::Up,
                },
            ),
            (
                "focus-pane-right",
                &self.focus_pane_right,
                Action::FocusPane {
                    direction: Direction::Right,
                },
            ),
            (
                "split-pane-vertical",
                &self.split_pane_vertical,
                Action::SplitPane {
                    direction: SplitDirection::Vertical,
                },
            ),
            (
                "split-pane-horizontal",
                &self.split_pane_horizontal,
                Action::SplitPane {
                    direction: SplitDirection::Horizontal,
                },
            ),
            (
                "swap-pane-prev",
                &self.swap_pane_prev,
                Action::SwapPane {
                    offset: SwapOffset::Prev,
                },
            ),
            (
                "swap-pane-next",
                &self.swap_pane_next,
                Action::SwapPane {
                    offset: SwapOffset::Next,
                },
            ),
            (
                "close-activity",
                &self.close_activity,
                Action::CloseActivity,
            ),
            (
                "new-terminal-activity",
                &self.new_terminal_activity,
                Action::NewTerminalActivity,
            ),
            (
                "focus-activity-prev",
                &self.focus_activity_prev,
                Action::FocusActivity {
                    offset: ActivityOffset::Prev,
                },
            ),
            (
                "focus-activity-next",
                &self.focus_activity_next,
                Action::FocusActivity {
                    offset: ActivityOffset::Next,
                },
            ),
            (
                "enter-copy-mode",
                &self.enter_copy_mode,
                Action::EnterCopyMode,
            ),
            ("new-window", &self.new_window, Action::NewWindow),
            (
                "focus-window-prev",
                &self.focus_window_prev,
                Action::FocusWindow {
                    offset: WindowOffset::Prev,
                },
            ),
            (
                "focus-window-next",
                &self.focus_window_next,
                Action::FocusWindow {
                    offset: WindowOffset::Next,
                },
            ),
        ]
        .into_iter()
    }

    /// KeyChord -> Action reverse lookup. Linear scan of 17 entries.
    /// Hot path; cheap given the fixed size. HashMap caching deferred per spec.
    pub fn lookup(&self, chord: &KeyChord) -> Option<Action> {
        self.iter().find_map(|(_, bound, action)| {
            if bound.as_ref() == Some(chord) {
                Some(action)
            } else {
                None
            }
        })
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

/// All shortcut actions supported by ozmux v0.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    /// Close the active pane.
    ClosePane,
    /// Close the active window.
    CloseWindow,
    /// Close the active session.
    CloseSession,
    /// Move pane focus in a direction.
    FocusPane {
        /// Direction to move focus.
        direction: Direction,
    },
    /// Move window focus by offset.
    FocusWindow {
        /// Window offset to apply.
        offset: WindowOffset,
    },
    /// Jump to a window by index.
    FocusWindowNumber {
        /// Target window index (0–9 in practice).
        index: u8,
    },
    /// Move activity focus by offset.
    FocusActivity {
        /// Activity offset to apply.
        offset: ActivityOffset,
    },
    /// Split the active pane.
    SplitPane {
        /// Split direction.
        direction: SplitDirection,
    },
    /// Split the active pane and move the active activity into the new pane.
    BreakActivityToPane {
        /// Split direction.
        direction: SplitDirection,
    },
    /// Create a new window.
    NewWindow,
    /// Create a new session.
    NewSession,
    /// Add a new terminal activity to the active pane.
    NewTerminalActivity,
    /// Add a new extension activity to the active pane.
    NewExtensionActivity,
    /// Rename the active session.
    RenameSession,
    /// Rename the active window.
    RenameWindow,
    /// Rename the active activity.
    RenameActivity,
    /// Close the active activity.
    CloseActivity,
    /// Show the window list.
    ListWindows,
    /// Show the activity list for the active pane.
    ListActivities,
    /// Resize the active pane in a direction.
    ResizePane {
        /// Resize direction.
        direction: Direction,
    },
    /// Toggle zoom/maximize on the active pane.
    ZoomPane,
    /// Swap the active pane with a sibling.
    SwapPane {
        /// Swap offset.
        offset: SwapOffset,
    },
    /// Break the active pane into a new window.
    BreakPaneToWindow,
    /// Show the cross-session window picker (tmux choose-tree).
    ChooseTree,
    /// Enter tmux-style copy mode on the active Terminal Activity.
    EnterCopyMode,
}

/// Layout direction shared by `FocusPane` and `ResizePane`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    /// Up.
    Up,
    /// Down.
    Down,
    /// Left.
    Left,
    /// Right.
    Right,
}

/// Split orientation for `SplitPane`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SplitDirection {
    /// New pane to the right of the current one.
    Horizontal,
    /// New pane below the current one.
    Vertical,
}

/// Window offset selectors for `FocusWindow`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WindowOffset {
    /// Next window.
    Next,
    /// Previous window.
    Prev,
    /// Last-active window.
    Last,
}

/// Activity offset selectors for `FocusActivity`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityOffset {
    /// Next activity in the active pane.
    Next,
    /// Previous activity in the active pane.
    Prev,
    /// Last-active activity in the active pane.
    Last,
}

/// Swap offset selectors for `SwapPane`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SwapOffset {
    /// Swap with the previous sibling.
    Prev,
    /// Swap with the next sibling.
    Next,
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
    fn bindings_default_has_all_17_fields_some() {
        let b = Bindings::default();
        assert!(b.close_pane.is_some());
        assert!(b.focus_pane_left.is_some());
        assert!(b.focus_pane_down.is_some());
        assert!(b.focus_pane_up.is_some());
        assert!(b.focus_pane_right.is_some());
        assert!(b.split_pane_vertical.is_some());
        assert!(b.split_pane_horizontal.is_some());
        assert!(b.swap_pane_prev.is_some());
        assert!(b.swap_pane_next.is_some());
        assert!(b.close_activity.is_some());
        assert!(b.new_terminal_activity.is_some());
        assert!(b.focus_activity_prev.is_some());
        assert!(b.focus_activity_next.is_some());
        assert!(b.enter_copy_mode.is_some());
        assert!(b.new_window.is_some());
        assert!(b.focus_window_prev.is_some());
        assert!(b.focus_window_next.is_some());
    }

    #[test]
    fn bindings_default_focus_pane_left_is_cmd_h() {
        let b = Bindings::default();
        let chord = b.focus_pane_left.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('h'));
        assert!(chord.modifiers.meta);
        assert!(!chord.modifiers.shift);
    }

    #[test]
    fn bindings_default_close_pane_is_cmd_shift_d() {
        let b = Bindings::default();
        let chord = b.close_pane.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('d'));
        assert!(chord.modifiers.meta);
        assert!(chord.modifiers.shift);
    }

    #[test]
    fn lookup_default_cmd_j_returns_focus_pane_down() {
        let b = Bindings::default();
        let chord = parse_key_chord("Cmd+J").unwrap();
        let action = b.lookup(&chord).expect("Cmd+J must resolve");
        assert!(matches!(
            action,
            Action::FocusPane {
                direction: Direction::Down
            }
        ));
    }

    #[test]
    fn lookup_unbound_chord_returns_none() {
        let b = Bindings::default();
        let chord = parse_key_chord("Cmd+Shift+Z").unwrap();
        assert!(b.lookup(&chord).is_none());
    }

    #[test]
    fn lookup_after_field_unbind_returns_none() {
        let mut b = Bindings::default();
        let chord = b.close_pane.clone().unwrap();
        b.close_pane = None;
        assert!(b.lookup(&chord).is_none());
    }

    #[test]
    fn validate_no_conflicts_default_ok() {
        let b = Bindings::default();
        assert!(b.validate_no_conflicts().is_ok());
    }

    #[test]
    fn validate_no_conflicts_detects_user_conflict() {
        let mut b = Bindings::default();
        b.close_pane = Some(parse_key_chord("Cmd+J").unwrap());
        let err = b.validate_no_conflicts().unwrap_err();
        assert_eq!(err.len(), 1, "exactly one duplicate-chord entry");
        assert!(err[0].actions.contains(&"close-pane"));
        assert!(err[0].actions.contains(&"focus-pane-down"));
    }

    #[test]
    fn iter_yields_17_entries() {
        let b = Bindings::default();
        assert_eq!(b.iter().count(), 17);
    }

    #[test]
    fn default_shortcuts_json_snapshot() {
        let json = serde_json::to_string(&Shortcuts::default()).unwrap();
        // The Bindings struct serializes its fields in declaration order.
        // The kebab-case rename applies. Any change to defaults updates this string.
        let expected = r#"{"bindings":{"close-pane":{"key":"d","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":true}},"focus-pane-left":{"key":"h","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"focus-pane-down":{"key":"j","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"focus-pane-up":{"key":"k","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"focus-pane-right":{"key":"l","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"split-pane-vertical":{"key":"i","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"split-pane-horizontal":{"key":"o","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"swap-pane-prev":{"key":"b","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"swap-pane-next":{"key":"n","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"close-activity":{"key":"f","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":true}},"new-terminal-activity":{"key":"t","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"focus-activity-prev":{"key":"[","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"focus-activity-next":{"key":"]","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"enter-copy-mode":{"key":"u","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"new-window":{"key":"r","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"focus-window-prev":{"key":"[","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":true}},"focus-window-next":{"key":"]","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":true}}}}"#;
        assert_eq!(json, expected);
    }
}
