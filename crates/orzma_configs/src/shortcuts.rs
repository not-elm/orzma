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
    /// True when this logical key resolves to a physical `KeyCode` at runtime,
    /// so a leader bound to it can actually fire.
    ///
    /// # Invariants
    ///
    /// The accepted domain MUST mirror `key_to_keycode` in
    /// `src/input/shortcuts.rs` exactly: an ASCII-alphanumeric `Char` and the
    /// named keys below map; `Plus`, `Other`, and any non-alphanumeric `Char`
    /// do not. A divergence would let an unmappable leader pass config
    /// validation yet resolve to no `KeyCode`, silently disabling the whole
    /// prefix table.
    pub fn maps_to_physical_key(&self) -> bool {
        // NOTE: keep this domain in lockstep with `key_to_keycode`
        // (src/input/shortcuts.rs); a divergence silently disables the prefix
        // table (see the invariant above).
        match self {
            Key::Char(c) => c.is_ascii_alphanumeric() || matches!(c, '[' | ']'),
            Key::Escape
            | Key::Space
            | Key::Enter
            | Key::Tab
            | Key::Backspace
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight => true,
            Key::Plus | Key::Other(_) => false,
        }
    }

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
/// `deser_binding_or_unbind` handles the unbind case before calling this.
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

/// One chord-collision entry. Carried inside
/// `OrzmaConfigsError::DuplicateChords` (defined in `error.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateChord {
    /// The chord that has multiple bindings.
    pub chord: KeyChord,
    /// Action labels (kebab-case TOML keys) that share this chord. Length >= 2.
    pub actions: Vec<&'static str>,
}

/// A bare modifier that can act as a tap leader. `Shift` is intentionally
/// excluded (too noisy as a tap).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapModifier {
    /// `Cmd` / `Command` / `Meta` / `Super`.
    Meta,
    /// `Ctrl`.
    Ctrl,
    /// `Alt` / `Opt` / `Option`.
    Alt,
}

/// The leader in either form: a key-containing chord, or a bare modifier tap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leader {
    /// A chord leader (`Ctrl+A`): press+release the chord, then the next key.
    Chord(KeyChord),
    /// A modifier-tap leader (`Cmd`): tap the bare modifier, then the next key.
    ModifierTap(TapModifier),
}

/// A resolved shortcut binding: a direct chord, or a leader-scoped chord
/// reached after the configured `leader`. The `<Leader>` token in a config
/// value selects the `Leader` variant.
///
/// serde derive is intentionally absent: the string grammar `"Cmd+V"` /
/// `"<Leader>s"` is (de)serialized by the field functions above (a derived
/// enum would emit externally-tagged output, not the string form).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Fires when the chord is pressed directly.
    Direct(KeyChord),
    /// Fires when the chord is pressed after the leader (`<Leader>x`), or —
    /// with `repeat` — re-fires inside the repeat window (`<Leader:r>x`).
    Leader {
        /// The second-key chord pressed after the leader.
        chord: KeyChord,
        /// True when bound with the `<Leader:r>` token: after firing, the key
        /// re-fires within `repeat-time-ms` without re-pressing the leader.
        repeat: bool,
    },
}

impl Binding {
    /// The chord to match: the direct chord, or the second-key chord for a
    /// leader-scoped binding.
    pub fn chord(&self) -> &KeyChord {
        match self {
            Binding::Direct(chord) | Binding::Leader { chord, .. } => chord,
        }
    }
}

/// User-facing shortcut configuration: the leader chord plus one flat binding
/// per action. Each value is a chord string (`"Cmd+V"`), a leader-scoped chord
/// (`"<Leader>s"`), or `""` (unbind). The `kebab-case` rename maps each TOML
/// key to its field; struct-level `#[serde(default)]` + `impl Default` seed
/// omitted actions from their active defaults; `deny_unknown_fields` rejects
/// typos (and the retired `[shortcuts.bindings]` / `[shortcuts.prefix_bindings]`
/// tables and the old `prefix` key) at load time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Shortcuts {
    /// The leader for `<Leader>`-scoped bindings: a chord (`Ctrl+A`) or a bare
    /// modifier tap (`Cmd`). Empty/absent = disabled.
    #[serde(deserialize_with = "deser_leader", serialize_with = "ser_leader")]
    pub leader: Option<Leader>,
    /// Paste the system clipboard into the active terminal.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub paste: Option<Binding>,
    /// Copy the focused terminal's selection to the system clipboard.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub copy: Option<Binding>,
    /// Release keyboard focus from a focused webview back to the terminal.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub release_webview_focus: Option<Binding>,
    /// Quit the orzma application.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub quit: Option<Binding>,
    /// Enter vi mode: Alacritty vi mode in `AppMode::Default`, tmux
    /// vi-mode in `AppMode::Tmux`.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub enter_vi_mode: Option<Binding>,
    /// Detach the current tmux session and switch to Default mode.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub detach_session: Option<Binding>,
    /// Focus the pane to the left (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_left_pane: Option<Binding>,
    /// Focus the pane below (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_down_pane: Option<Binding>,
    /// Focus the pane above (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_up_pane: Option<Binding>,
    /// Focus the pane to the right (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_right_pane: Option<Binding>,
    /// Split the active pane side-by-side — vertical divider, tmux `-h` (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub split_vertical_pane: Option<Binding>,
    /// Split the active pane stacked — horizontal divider, tmux `-v` (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub split_horizontal_pane: Option<Binding>,
    /// Kill the active pane, after a confirm prompt (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub kill_pane: Option<Binding>,
    /// Toggle zoom on the active pane (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub zoom_pane: Option<Binding>,
    /// Resize the active pane's border left by 5 cells, repeatable (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_left_pane: Option<Binding>,
    /// Resize the active pane's border down by 5 cells, repeatable (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_down_pane: Option<Binding>,
    /// Resize the active pane's border up by 5 cells, repeatable (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_up_pane: Option<Binding>,
    /// Resize the active pane's border right by 5 cells, repeatable (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_right_pane: Option<Binding>,
    /// Open a new window (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub new_window: Option<Binding>,
    /// Kill the active window, after a confirm prompt (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub kill_window: Option<Binding>,
    /// Switch to the next window (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub next_window: Option<Binding>,
    /// Switch to the previous window (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub previous_window: Option<Binding>,
    /// Switch to the next session (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub next_session: Option<Binding>,
    /// Switch to the previous session (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub previous_session: Option<Binding>,
    /// Switch to the window at tmux index 0 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_0: Option<Binding>,
    /// Switch to the window at tmux index 1 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_1: Option<Binding>,
    /// Switch to the window at tmux index 2 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_2: Option<Binding>,
    /// Switch to the window at tmux index 3 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_3: Option<Binding>,
    /// Switch to the window at tmux index 4 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_4: Option<Binding>,
    /// Switch to the window at tmux index 5 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_5: Option<Binding>,
    /// Switch to the window at tmux index 6 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_6: Option<Binding>,
    /// Switch to the window at tmux index 7 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_7: Option<Binding>,
    /// Switch to the window at tmux index 8 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_8: Option<Binding>,
    /// Switch to the window at tmux index 9 (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_window_9: Option<Binding>,
    /// Open the rename prompt for the active window (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub rename_window: Option<Binding>,
    /// Open the rename prompt for the session (tmux mode only).
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub rename_session: Option<Binding>,
    /// Timeout (ms) for a modifier-tap leader: press+release within this window
    /// with no intervening key/mouse press counts as a tap. Default 300; 0 is
    /// normalized to 300.
    pub leader_tap_timeout_ms: u64,
    /// Repeat window (ms) for `<Leader:r>` bindings: after such a binding
    /// fires, pressing a repeat-marked key again within this window re-fires
    /// it without the leader; each fire re-arms the window. Default 500.
    ///
    /// 0 disables repeat entirely (tmux `repeat-time 0` parity) and is NOT
    /// normalized away, unlike `leader_tap_timeout_ms`.
    pub repeat_time_ms: u64,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Shortcuts {
            leader: Some(Leader::ModifierTap(TapModifier::Meta)),
            paste: Some(Binding::Direct(parse_default_chord("Cmd+V"))),
            copy: Some(Binding::Direct(parse_default_chord("Cmd+C"))),
            release_webview_focus: Some(parse_default_binding("<Leader>u")),
            quit: Some(Binding::Direct(parse_default_chord("Cmd+Q"))),
            enter_vi_mode: Some(parse_default_binding("<Leader>s")),
            detach_session: Some(parse_default_binding("<Leader>x")),
            select_left_pane: Some(parse_default_binding("<Leader>h")),
            select_down_pane: Some(parse_default_binding("<Leader>j")),
            select_up_pane: Some(parse_default_binding("<Leader>k")),
            select_right_pane: Some(parse_default_binding("<Leader>l")),
            split_vertical_pane: Some(parse_default_binding("<Leader>i")),
            split_horizontal_pane: Some(parse_default_binding("<Leader>o")),
            kill_pane: Some(parse_default_binding("<Leader>p")),
            zoom_pane: Some(parse_default_binding("<Leader>z")),
            resize_left_pane: Some(parse_default_binding("<Leader:r>Shift+H")),
            resize_down_pane: Some(parse_default_binding("<Leader:r>Shift+J")),
            resize_up_pane: Some(parse_default_binding("<Leader:r>Shift+K")),
            resize_right_pane: Some(parse_default_binding("<Leader:r>Shift+L")),
            new_window: Some(parse_default_binding("<Leader>c")),
            kill_window: Some(parse_default_binding("<Leader>Shift+X")),
            next_window: Some(parse_default_binding("<Leader>]")),
            previous_window: Some(parse_default_binding("<Leader>[")),
            next_session: Some(parse_default_binding("<Leader>Shift+]")),
            previous_session: Some(parse_default_binding("<Leader>Shift+[")),
            select_window_0: Some(parse_default_binding("<Leader>0")),
            select_window_1: Some(parse_default_binding("<Leader>1")),
            select_window_2: Some(parse_default_binding("<Leader>2")),
            select_window_3: Some(parse_default_binding("<Leader>3")),
            select_window_4: Some(parse_default_binding("<Leader>4")),
            select_window_5: Some(parse_default_binding("<Leader>5")),
            select_window_6: Some(parse_default_binding("<Leader>6")),
            select_window_7: Some(parse_default_binding("<Leader>7")),
            select_window_8: Some(parse_default_binding("<Leader>8")),
            select_window_9: Some(parse_default_binding("<Leader>9")),
            rename_window: Some(parse_default_binding("<Leader>r")),
            rename_session: Some(parse_default_binding("<Leader>Shift+R")),
            leader_tap_timeout_ms: 300,
            repeat_time_ms: 500,
        }
    }
}

impl Shortcuts {
    /// `(label, &Option<Binding>, action)` for every action, in stable order.
    /// The single source of truth for the action schema.
    pub fn bindings_iter(
        &self,
    ) -> impl Iterator<Item = (&'static str, &Option<Binding>, Shortcut)> + '_ {
        [
            ("paste", &self.paste, Shortcut::Paste),
            ("copy", &self.copy, Shortcut::Copy),
            (
                "release-webview-focus",
                &self.release_webview_focus,
                Shortcut::ReleaseWebviewFocus,
            ),
            ("quit", &self.quit, Shortcut::Quit),
            ("enter-vi-mode", &self.enter_vi_mode, Shortcut::EnterViMode),
            (
                "detach-session",
                &self.detach_session,
                Shortcut::DetachSession,
            ),
            (
                "select-left-pane",
                &self.select_left_pane,
                Shortcut::SelectPane(PaneDirection::Left),
            ),
            (
                "select-down-pane",
                &self.select_down_pane,
                Shortcut::SelectPane(PaneDirection::Down),
            ),
            (
                "select-up-pane",
                &self.select_up_pane,
                Shortcut::SelectPane(PaneDirection::Up),
            ),
            (
                "select-right-pane",
                &self.select_right_pane,
                Shortcut::SelectPane(PaneDirection::Right),
            ),
            (
                "split-vertical-pane",
                &self.split_vertical_pane,
                Shortcut::SplitPane(SplitOrientation::Vertical),
            ),
            (
                "split-horizontal-pane",
                &self.split_horizontal_pane,
                Shortcut::SplitPane(SplitOrientation::Horizontal),
            ),
            ("kill-pane", &self.kill_pane, Shortcut::KillPane),
            ("zoom-pane", &self.zoom_pane, Shortcut::ZoomPane),
            (
                "resize-left-pane",
                &self.resize_left_pane,
                Shortcut::ResizePane(PaneDirection::Left),
            ),
            (
                "resize-down-pane",
                &self.resize_down_pane,
                Shortcut::ResizePane(PaneDirection::Down),
            ),
            (
                "resize-up-pane",
                &self.resize_up_pane,
                Shortcut::ResizePane(PaneDirection::Up),
            ),
            (
                "resize-right-pane",
                &self.resize_right_pane,
                Shortcut::ResizePane(PaneDirection::Right),
            ),
            ("new-window", &self.new_window, Shortcut::NewWindow),
            ("kill-window", &self.kill_window, Shortcut::KillWindow),
            ("next-window", &self.next_window, Shortcut::NextWindow),
            (
                "previous-window",
                &self.previous_window,
                Shortcut::PreviousWindow,
            ),
            ("next-session", &self.next_session, Shortcut::NextSession),
            (
                "previous-session",
                &self.previous_session,
                Shortcut::PreviousSession,
            ),
            (
                "select-window-0",
                &self.select_window_0,
                Shortcut::SelectWindow(0),
            ),
            (
                "select-window-1",
                &self.select_window_1,
                Shortcut::SelectWindow(1),
            ),
            (
                "select-window-2",
                &self.select_window_2,
                Shortcut::SelectWindow(2),
            ),
            (
                "select-window-3",
                &self.select_window_3,
                Shortcut::SelectWindow(3),
            ),
            (
                "select-window-4",
                &self.select_window_4,
                Shortcut::SelectWindow(4),
            ),
            (
                "select-window-5",
                &self.select_window_5,
                Shortcut::SelectWindow(5),
            ),
            (
                "select-window-6",
                &self.select_window_6,
                Shortcut::SelectWindow(6),
            ),
            (
                "select-window-7",
                &self.select_window_7,
                Shortcut::SelectWindow(7),
            ),
            (
                "select-window-8",
                &self.select_window_8,
                Shortcut::SelectWindow(8),
            ),
            (
                "select-window-9",
                &self.select_window_9,
                Shortcut::SelectWindow(9),
            ),
            ("rename-window", &self.rename_window, Shortcut::RenameWindow),
            (
                "rename-session",
                &self.rename_session,
                Shortcut::RenameSession,
            ),
        ]
        .into_iter()
    }

    /// Bound direct chords only: `(label, chord, action)`.
    pub fn direct_chords(&self) -> impl Iterator<Item = (&'static str, &KeyChord, Shortcut)> + '_ {
        self.bindings_iter()
            .filter_map(|(label, bound, action)| match bound {
                Some(Binding::Direct(chord)) => Some((label, chord, action)),
                _ => None,
            })
    }

    /// Bound leader-scoped chords only: `(label, chord, action, repeat)`.
    pub fn leader_chords(
        &self,
    ) -> impl Iterator<Item = (&'static str, &KeyChord, Shortcut, bool)> + '_ {
        self.bindings_iter()
            .filter_map(|(label, bound, action)| match bound {
                Some(Binding::Leader { chord, repeat }) => Some((label, chord, action, *repeat)),
                _ => None,
            })
    }

    /// Detects chord collisions among direct bindings.
    pub(crate) fn validate_no_direct_conflicts(&self) -> Result<(), Vec<DuplicateChord>> {
        conflicts(self.direct_chords())
    }

    /// Detects chord collisions among leader-scoped bindings.
    pub(crate) fn validate_no_leader_conflicts(&self) -> Result<(), Vec<DuplicateChord>> {
        conflicts(
            self.leader_chords()
                .map(|(label, chord, action, _)| (label, chord, action)),
        )
    }

    /// Normalizes numeric fields: a `leader_tap_timeout_ms` of 0 is meaningless
    /// (a tap would never fit), so it reverts to the 300 default.
    pub(crate) fn normalize(&mut self) {
        if self.leader_tap_timeout_ms == 0 {
            self.leader_tap_timeout_ms = 300;
        }
    }
}

/// A neighbor direction for the `select-pane` shortcut actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDirection {
    /// Focus the pane to the left.
    Left,
    /// Focus the pane below.
    Down,
    /// Focus the pane above.
    Up,
    /// Focus the pane to the right.
    Right,
}

/// Which way a split divides the pane, named after the DIVIDER the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitOrientation {
    /// A vertical divider: panes end up side by side (tmux `split-window -h`).
    Vertical,
    /// A horizontal divider: panes end up stacked (tmux `split-window -v`).
    Horizontal,
}

/// Shortcut actions. GUI-local actions plus the tmux pane/window operations
/// (the latter are inert outside `AppMode::Tmux`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// Paste the system clipboard into the active terminal.
    Paste,
    /// Copies the focused terminal's selection to the system clipboard.
    Copy,
    /// Releases keyboard focus from a focused webview back to the terminal.
    ReleaseWebviewFocus,
    /// Quits the orzma application.
    Quit,
    /// Detaches from the tmux session and returns to Default single-terminal mode.
    DetachSession,
    /// Enters vi mode: Alacritty vi mode in `AppMode::Default`, tmux
    /// vi-mode in `AppMode::Tmux`.
    EnterViMode,
    /// Focuses the neighbor pane in the given direction (tmux mode only).
    SelectPane(PaneDirection),
    /// Splits the active pane (tmux mode only).
    SplitPane(SplitOrientation),
    /// Kills the active pane after a confirm prompt (tmux mode only).
    KillPane,
    /// Toggles zoom on the active pane (tmux mode only).
    ZoomPane,
    /// Resizes the active pane's border in the given direction (tmux mode only).
    ResizePane(PaneDirection),
    /// Opens a new window in the current session (tmux mode only).
    NewWindow,
    /// Kills the active window after a confirm prompt (tmux mode only).
    KillWindow,
    /// Switches to the next window (tmux mode only).
    NextWindow,
    /// Switches to the previous window (tmux mode only).
    PreviousWindow,
    /// Switches the tmux client to the next session (tmux mode only).
    NextSession,
    /// Switches the tmux client to the previous session (tmux mode only).
    PreviousSession,
    /// Switches to the window with this tmux display index (tmux mode only).
    SelectWindow(u8),
    /// Opens the rename prompt for the active window (tmux mode only).
    RenameWindow,
    /// Opens the rename prompt for the session (tmux mode only).
    RenameSession,
}

/// The kebab-case TOML key for every `Shortcuts` action field (`leader` and
/// the two `_ms` timeout scalars excluded), in `bindings_iter` order. The
/// single source of truth `crate::resolve` diffs a user's `[shortcuts]`
/// binding map against; kept honest by the `every_action_key_routes` test
/// below.
pub(crate) const SHORTCUT_ACTION_KEYS: &[&str] = &[
    "paste",
    "copy",
    "release-webview-focus",
    "quit",
    "enter-vi-mode",
    "detach-session",
    "select-left-pane",
    "select-down-pane",
    "select-up-pane",
    "select-right-pane",
    "split-vertical-pane",
    "split-horizontal-pane",
    "kill-pane",
    "zoom-pane",
    "resize-left-pane",
    "resize-down-pane",
    "resize-up-pane",
    "resize-right-pane",
    "new-window",
    "kill-window",
    "next-window",
    "previous-window",
    "next-session",
    "previous-session",
    "select-window-0",
    "select-window-1",
    "select-window-2",
    "select-window-3",
    "select-window-4",
    "select-window-5",
    "select-window-6",
    "select-window-7",
    "select-window-8",
    "select-window-9",
    "rename-window",
    "rename-session",
];

/// The literal token marking a leader-scoped binding value (`<Leader>x`).
/// Matched case-insensitively at the START of the value only.
const LEADER_TOKEN: &str = "<Leader>";

/// The literal token marking a repeatable leader-scoped binding value
/// (`<Leader:r>x`). Matched case-insensitively at the START of the value only.
const LEADER_REPEAT_TOKEN: &str = "<Leader:r>";

/// Strips a leading, case-insensitive `<Leader>` / `<Leader:r>` token,
/// returning the remaining chord text and whether the repeat form was used.
/// `None` when the value is not leader-scoped.
fn strip_leader_prefix(value: &str) -> Option<(&str, bool)> {
    if let Some((head, rest)) = value.split_at_checked(LEADER_REPEAT_TOKEN.len())
        && head.eq_ignore_ascii_case(LEADER_REPEAT_TOKEN)
    {
        return Some((rest, true));
    }
    let (head, rest) = value.split_at_checked(LEADER_TOKEN.len())?;
    head.eq_ignore_ascii_case(LEADER_TOKEN)
        .then_some((rest, false))
}

/// Parses a non-empty config value into a `Binding`: a leading `<Leader>`
/// or `<Leader:r>` selects `Leader`, with the `:r` form setting
/// `repeat: true`; otherwise the value parses as `Direct`. The remainder is
/// parsed by `parse_key_chord`, so `"<Leader>"` (empty remainder) is an
/// error.
pub(crate) fn parse_binding(value: &str) -> Result<Binding, KeyChordParseError> {
    match strip_leader_prefix(value) {
        Some((rest, repeat)) => {
            parse_key_chord(rest).map(|chord| Binding::Leader { chord, repeat })
        }
        None => parse_key_chord(value).map(Binding::Direct),
    }
}

/// serde field deserializer for `Option<Binding>`: empty string is unbind
/// (`None`); any other string parses via `parse_binding`.
fn deser_binding_or_unbind<'de, D>(d: D) -> Result<Option<Binding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Ok(None);
    }
    parse_binding(&s).map(Some).map_err(DeError::custom)
}

/// serde field serializer for `Option<Binding>`: `None` → `""`,
/// `Direct(c)` → `c`, `Leader { chord, repeat: false }` → `"<Leader>" + c`,
/// `Leader { chord, repeat: true }` → `"<Leader:r>" + c`.
fn ser_binding_or_unbind<S>(value: &Option<Binding>, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let text = match value {
        None => String::new(),
        Some(Binding::Direct(chord)) => chord.to_string(),
        Some(Binding::Leader { chord, repeat }) => {
            let token = if *repeat {
                LEADER_REPEAT_TOKEN
            } else {
                LEADER_TOKEN
            };
            format!("{token}{chord}")
        }
    };
    ser.serialize_str(&text)
}

/// If `value` is exactly one allowed tap-modifier token (case-insensitive),
/// returns it. Returns `None` for `Shift`, any `+`-joined chord, or a
/// non-modifier token, so those fall through to chord parsing.
fn single_tap_modifier(value: &str) -> Option<TapModifier> {
    let (mods, _) = parse_modifier_to_bit(value)?;
    if mods.meta {
        Some(TapModifier::Meta)
    } else if mods.ctrl {
        Some(TapModifier::Ctrl)
    } else if mods.alt {
        Some(TapModifier::Alt)
    } else {
        None
    }
}

/// The canonical token a `TapModifier` serializes to.
fn tap_modifier_token(m: TapModifier) -> &'static str {
    match m {
        TapModifier::Meta => "Cmd",
        TapModifier::Ctrl => "Ctrl",
        TapModifier::Alt => "Alt",
    }
}

/// Parses a non-empty `leader` value: a single allowed tap-modifier token →
/// `ModifierTap`; anything else → `parse_key_chord` → `Chord`. A bare `Shift`
/// (and any other bare modifier) errors via `parse_key_chord` (no key).
pub(crate) fn parse_leader(value: &str) -> Result<Leader, KeyChordParseError> {
    if let Some(m) = single_tap_modifier(value) {
        return Ok(Leader::ModifierTap(m));
    }
    parse_key_chord(value).map(Leader::Chord)
}

/// serde field deserializer for the leader: empty string → `None`; a bare
/// tap-modifier → `ModifierTap`; otherwise a chord.
fn deser_leader<'de, D>(d: D) -> Result<Option<Leader>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Ok(None);
    }
    parse_leader(&s).map(Some).map_err(DeError::custom)
}

/// serde field serializer for the leader: `None` → `""`, `Chord(c)` → `c`,
/// `ModifierTap(m)` → its canonical token.
fn ser_leader<S>(value: &Option<Leader>, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let text = match value {
        None => String::new(),
        Some(Leader::Chord(chord)) => chord.to_string(),
        Some(Leader::ModifierTap(m)) => tap_modifier_token(*m).to_string(),
    };
    ser.serialize_str(&text)
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

fn parse_default_chord(s: &str) -> KeyChord {
    parse_key_chord(s).unwrap_or_else(|e| panic!("invalid default chord {s:?}: {e}"))
}

fn parse_default_binding(s: &str) -> Binding {
    parse_binding(s).unwrap_or_else(|e| panic!("invalid default binding {s:?}: {e}"))
}

/// Detects chord collisions across a table's bound entries. Returns a `Vec`
/// sorted by chord (BTreeMap key order) for deterministic error output.
fn conflicts<'a>(
    entries: impl Iterator<Item = (&'static str, &'a KeyChord, Shortcut)>,
) -> Result<(), Vec<DuplicateChord>> {
    let mut by_chord: BTreeMap<KeyChord, Vec<&'static str>> = BTreeMap::new();
    for (label, chord, _action) in entries {
        by_chord.entry(chord.clone()).or_default().push(label);
    }
    let dupes: Vec<DuplicateChord> = by_chord
        .into_iter()
        .filter(|(_, labels)| labels.len() >= 2)
        .map(|(chord, actions)| DuplicateChord { chord, actions })
        .collect();
    if dupes.is_empty() { Ok(()) } else { Err(dupes) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_physical_key_true_for_alphanumeric_and_named() {
        assert!(Key::Char('a').maps_to_physical_key());
        assert!(Key::Char('Z').maps_to_physical_key());
        assert!(Key::Char('7').maps_to_physical_key());
        assert!(Key::Char('[').maps_to_physical_key());
        assert!(Key::Char(']').maps_to_physical_key());
        assert!(Key::Escape.maps_to_physical_key());
        assert!(Key::Space.maps_to_physical_key());
        assert!(Key::Enter.maps_to_physical_key());
        assert!(Key::Tab.maps_to_physical_key());
        assert!(Key::Backspace.maps_to_physical_key());
        assert!(Key::ArrowUp.maps_to_physical_key());
        assert!(Key::ArrowDown.maps_to_physical_key());
        assert!(Key::ArrowLeft.maps_to_physical_key());
        assert!(Key::ArrowRight.maps_to_physical_key());
    }

    #[test]
    fn maps_to_physical_key_false_for_plus_other_and_punctuation() {
        assert!(!Key::Plus.maps_to_physical_key());
        assert!(!Key::Other("f12".into()).maps_to_physical_key());
        assert!(!Key::Char('.').maps_to_physical_key());
        assert!(!Key::Char('-').maps_to_physical_key());
    }

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
    fn strip_leader_prefix_only_at_start() {
        assert_eq!(strip_leader_prefix("<Leader>s"), Some(("s", false)));
        assert_eq!(
            strip_leader_prefix("<Leader>Ctrl+d"),
            Some(("Ctrl+d", false))
        );
        assert_eq!(strip_leader_prefix("Cmd+<Leader>"), None);
        assert_eq!(strip_leader_prefix("s"), None);
        assert_eq!(strip_leader_prefix(""), None);
    }

    #[test]
    fn parse_binding_direct_and_leader() {
        assert_eq!(
            parse_binding("Cmd+V").unwrap(),
            Binding::Direct(parse_key_chord("Cmd+V").unwrap())
        );
        assert_eq!(
            parse_binding("<Leader>s").unwrap(),
            Binding::Leader {
                chord: parse_key_chord("s").unwrap(),
                repeat: false,
            }
        );
        assert_eq!(
            parse_binding("<Leader>Ctrl+d").unwrap(),
            Binding::Leader {
                chord: parse_key_chord("Ctrl+d").unwrap(),
                repeat: false,
            }
        );
    }

    #[test]
    fn parse_binding_leader_token_case_insensitive() {
        let want = Binding::Leader {
            chord: parse_key_chord("s").unwrap(),
            repeat: false,
        };
        assert_eq!(parse_binding("<leader>s").unwrap(), want);
        assert_eq!(parse_binding("<LEADER>s").unwrap(), want);
    }

    #[test]
    fn parse_binding_empty_after_leader_is_err() {
        assert!(parse_binding("<Leader>").is_err());
    }

    #[test]
    fn binding_chord_extracts_inner() {
        assert_eq!(
            parse_binding("Cmd+V").unwrap().chord(),
            &parse_key_chord("Cmd+V").unwrap()
        );
        assert_eq!(
            parse_binding("<Leader>s").unwrap().chord(),
            &parse_key_chord("s").unwrap()
        );
    }

    #[derive(serde::Deserialize)]
    struct BindingWrapper {
        #[serde(deserialize_with = "deser_binding_or_unbind")]
        v: Option<Binding>,
    }

    #[test]
    fn deser_binding_empty_is_unbind() {
        let parsed: BindingWrapper = serde_json::from_str(r#"{"v":""}"#).unwrap();
        assert!(parsed.v.is_none());
    }

    #[test]
    fn deser_binding_leader_value() {
        let parsed: BindingWrapper = serde_json::from_str(r#"{"v":"<Leader>s"}"#).unwrap();
        assert_eq!(
            parsed.v,
            Some(Binding::Leader {
                chord: parse_key_chord("s").unwrap(),
                repeat: false,
            })
        );
    }

    #[test]
    fn shortcuts_default_is_active_direct_bindings() {
        let s = Shortcuts::default();
        assert_eq!(s.leader, Some(Leader::ModifierTap(TapModifier::Meta)));
        assert_eq!(
            s.paste,
            Some(Binding::Direct(parse_key_chord("Cmd+V").unwrap()))
        );
        assert_eq!(
            s.quit,
            Some(Binding::Direct(parse_key_chord("Cmd+Q").unwrap()))
        );
        assert_eq!(
            s.copy,
            Some(Binding::Direct(parse_key_chord("Cmd+C").unwrap()))
        );
        assert_eq!(s.bindings_iter().count(), 36);
        assert_eq!(s.direct_chords().count(), 3);
        assert_eq!(s.leader_chords().count(), 33);
    }

    #[test]
    fn bindings_iter_count_is_pinned_to_field_count() {
        // NOTE: drift guard — adding a Shortcuts field without its
        // bindings_iter() entry silently unbinds the action.
        assert_eq!(Shortcuts::default().bindings_iter().count(), 36);
    }

    #[test]
    fn every_action_key_routes() {
        // NOTE: one-directional drift guard — the loop below only checks
        // that listed keys route; this length check catches the other
        // direction, a new `Shortcuts` field added without a matching
        // `SHORTCUT_ACTION_KEYS` entry (which would silently make that
        // action unconfigurable from TOML).
        assert_eq!(
            SHORTCUT_ACTION_KEYS.len(),
            Shortcuts::default().bindings_iter().count()
        );
        // NOTE: `quit`'s own built-in default IS `Cmd+Q`, so overriding it
        // with that same chord would be a no-op and falsely fail this guard;
        // `Ctrl+Alt+Shift+9` is not any action's default (which use at most
        // one modifier, or none for a bare leader-scoped chord), so it is
        // guaranteed to change whichever field it lands on.
        for key in SHORTCUT_ACTION_KEYS {
            let t = format!("{key} = \"Ctrl+Alt+Shift+9\"\n");
            let s: Shortcuts = toml::from_str(&t).expect(key);
            assert_ne!(s, Shortcuts::default(), "key {key} did not route");
        }
    }

    #[test]
    fn default_resize_bindings_are_repeatable_shift_leader() {
        let s = Shortcuts::default();
        assert_eq!(
            s.resize_left_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("Shift+H").unwrap(),
                repeat: true
            })
        );
        assert_eq!(
            s.resize_right_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("Shift+L").unwrap(),
                repeat: true
            })
        );
    }

    #[test]
    fn default_tmux_actions_are_leader_bound() {
        let s = Shortcuts::default();
        assert_eq!(
            s.select_left_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("h").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.split_vertical_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("i").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.kill_window,
            Some(Binding::Leader {
                chord: parse_key_chord("Shift+X").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.select_window_0,
            Some(Binding::Leader {
                chord: parse_key_chord("0").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.rename_session,
            Some(Binding::Leader {
                chord: parse_key_chord("Shift+R").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.paste,
            Some(Binding::Direct(parse_key_chord("Cmd+V").unwrap()))
        );
    }

    #[test]
    fn tmux_actions_parse_from_flat_toml() {
        let toml = r#"
split-vertical-pane = "<Leader>g"
select-window-3 = ""
new-window = "Cmd+T"
"#;
        let s: Shortcuts = toml::from_str(toml).unwrap();
        assert_eq!(
            s.split_vertical_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("g").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(s.select_window_3, None);
        assert_eq!(
            s.new_window,
            Some(Binding::Direct(parse_key_chord("Cmd+T").unwrap()))
        );
    }

    #[test]
    fn default_shortcuts_has_no_conflicts() {
        let s = Shortcuts::default();
        assert!(s.validate_no_direct_conflicts().is_ok());
        assert!(s.validate_no_leader_conflicts().is_ok());
    }

    #[test]
    fn shortcuts_parses_flat_leader_and_bindings() {
        let toml = r#"
leader = "Ctrl+A"
enter-vi-mode = "<Leader>s"
detach-session = "<Leader>d"
"#;
        let s: Shortcuts = toml::from_str(toml).unwrap();
        assert_eq!(
            s.leader,
            Some(Leader::Chord(parse_key_chord("Ctrl+A").unwrap()))
        );
        assert_eq!(
            s.enter_vi_mode,
            Some(Binding::Leader {
                chord: parse_key_chord("s").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.detach_session,
            Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(
            s.paste,
            Some(Binding::Direct(parse_key_chord("Cmd+V").unwrap()))
        );
        assert_eq!(s.leader_chords().count(), 33);
    }

    #[test]
    fn shortcuts_rejects_unknown_field() {
        assert!(toml::from_str::<Shortcuts>("resize-pane-down = \"d\"\n").is_err());
    }

    #[test]
    fn direct_conflict_detected() {
        let s = Shortcuts {
            paste: Some(Binding::Direct(parse_key_chord("Cmd+Q").unwrap())),
            ..Default::default()
        };
        let err = s.validate_no_direct_conflicts().unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].actions.contains(&"paste"));
        assert!(err[0].actions.contains(&"quit"));
    }

    #[test]
    fn leader_conflict_detected() {
        let s = Shortcuts {
            enter_vi_mode: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            detach_session: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let err = s.validate_no_leader_conflicts().unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].actions.contains(&"enter-vi-mode"));
        assert!(err[0].actions.contains(&"detach-session"));
    }

    #[test]
    fn leader_conflict_detected_across_repeat_flag() {
        let s = Shortcuts {
            enter_vi_mode: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: true,
            }),
            detach_session: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let err = s.validate_no_leader_conflicts().unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].actions.contains(&"enter-vi-mode"));
        assert!(err[0].actions.contains(&"detach-session"));
    }

    #[test]
    fn default_shortcuts_json_snapshot() {
        let json = serde_json::to_string(&Shortcuts::default()).unwrap();
        let expected = r#"{"leader":"Cmd","paste":"Cmd+V","copy":"Cmd+C","release-webview-focus":"<Leader>U","quit":"Cmd+Q","enter-vi-mode":"<Leader>S","detach-session":"<Leader>X","select-left-pane":"<Leader>H","select-down-pane":"<Leader>J","select-up-pane":"<Leader>K","select-right-pane":"<Leader>L","split-vertical-pane":"<Leader>I","split-horizontal-pane":"<Leader>O","kill-pane":"<Leader>P","zoom-pane":"<Leader>Z","resize-left-pane":"<Leader:r>Shift+H","resize-down-pane":"<Leader:r>Shift+J","resize-up-pane":"<Leader:r>Shift+K","resize-right-pane":"<Leader:r>Shift+L","new-window":"<Leader>C","kill-window":"<Leader>Shift+X","next-window":"<Leader>]","previous-window":"<Leader>[","next-session":"<Leader>Shift+]","previous-session":"<Leader>Shift+[","select-window-0":"<Leader>0","select-window-1":"<Leader>1","select-window-2":"<Leader>2","select-window-3":"<Leader>3","select-window-4":"<Leader>4","select-window-5":"<Leader>5","select-window-6":"<Leader>6","select-window-7":"<Leader>7","select-window-8":"<Leader>8","select-window-9":"<Leader>9","rename-window":"<Leader>R","rename-session":"<Leader>Shift+R","leader-tap-timeout-ms":300,"repeat-time-ms":500}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn serialize_leader_binding_emits_leader_token() {
        let s = Shortcuts {
            leader: Some(Leader::Chord(parse_key_chord("Ctrl+A").unwrap())),
            detach_session: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains(r#""leader":"Ctrl+A""#),
            "leader serializes as its chord string; got {json}"
        );
        assert!(
            json.contains(r#""detach-session":"<Leader>D""#),
            "a Leader binding serializes with the <Leader> token; got {json}"
        );
    }

    #[test]
    fn parse_leader_bare_modifiers() {
        assert_eq!(
            parse_leader("Cmd").unwrap(),
            Leader::ModifierTap(TapModifier::Meta)
        );
        assert_eq!(
            parse_leader("command").unwrap(),
            Leader::ModifierTap(TapModifier::Meta)
        );
        assert_eq!(
            parse_leader("Super").unwrap(),
            Leader::ModifierTap(TapModifier::Meta)
        );
        assert_eq!(
            parse_leader("Ctrl").unwrap(),
            Leader::ModifierTap(TapModifier::Ctrl)
        );
        assert_eq!(
            parse_leader("Option").unwrap(),
            Leader::ModifierTap(TapModifier::Alt)
        );
    }

    #[test]
    fn parse_leader_chord_still_works() {
        assert_eq!(
            parse_leader("Ctrl+A").unwrap(),
            Leader::Chord(parse_key_chord("Ctrl+A").unwrap())
        );
    }

    #[test]
    fn parse_leader_rejects_bare_shift() {
        assert!(parse_leader("Shift").is_err());
    }

    #[test]
    fn shortcuts_parses_bare_modifier_leader_and_timeout() {
        let toml =
            "leader = \"Cmd\"\nleader-tap-timeout-ms = 250\ndetach-session = \"<Leader>d\"\n";
        let s: Shortcuts = toml::from_str(toml).unwrap();
        assert_eq!(s.leader, Some(Leader::ModifierTap(TapModifier::Meta)));
        assert_eq!(s.leader_tap_timeout_ms, 250);
    }

    #[test]
    fn shortcuts_leader_shift_is_parse_error() {
        assert!(toml::from_str::<Shortcuts>("leader = \"Shift\"\n").is_err());
    }

    #[test]
    fn shortcuts_default_timeout_is_300() {
        assert_eq!(Shortcuts::default().leader_tap_timeout_ms, 300);
    }

    #[test]
    fn normalize_clamps_zero_timeout_to_300() {
        let mut s = Shortcuts {
            leader_tap_timeout_ms: 0,
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.leader_tap_timeout_ms, 300);
    }

    #[test]
    fn serialize_bare_modifier_leader_emits_token() {
        let s = Shortcuts {
            leader: Some(Leader::ModifierTap(TapModifier::Meta)),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""leader":"Cmd""#), "got {json}");
    }

    #[test]
    fn strip_leader_prefix_detects_repeat_token() {
        assert_eq!(strip_leader_prefix("<Leader>s"), Some(("s", false)));
        assert_eq!(strip_leader_prefix("<Leader:r>s"), Some(("s", true)));
        assert_eq!(
            strip_leader_prefix("<leader:R>Ctrl+d"),
            Some(("Ctrl+d", true))
        );
        assert_eq!(strip_leader_prefix("s"), None);
        assert_eq!(strip_leader_prefix(""), None);
    }

    #[test]
    fn parse_binding_repeat_leader() {
        assert_eq!(
            parse_binding("<Leader:r>h").unwrap(),
            Binding::Leader {
                chord: parse_key_chord("h").unwrap(),
                repeat: true,
            }
        );
    }

    #[test]
    fn parse_binding_bare_repeat_token_is_err() {
        assert!(parse_binding("<Leader:r>").is_err());
    }

    #[test]
    fn serialize_repeat_leader_binding_emits_repeat_token() {
        let s = Shortcuts {
            detach_session: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: true,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains(r#""detach-session":"<Leader:r>D""#),
            "a repeat Leader binding serializes with the <Leader:r> token; got {json}"
        );
    }

    #[test]
    fn leader_chords_carries_repeat_flag() {
        let s = Shortcuts {
            enter_vi_mode: Some(Binding::Leader {
                chord: parse_key_chord("s").unwrap(),
                repeat: true,
            }),
            detach_session: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let entries: Vec<_> = s.leader_chords().collect();
        assert!(
            entries
                .iter()
                .any(|(l, _, _, r)| *l == "enter-vi-mode" && *r)
        );
        assert!(
            entries
                .iter()
                .any(|(l, _, _, r)| *l == "detach-session" && !*r)
        );
    }

    #[test]
    fn shortcuts_default_repeat_time_is_500() {
        assert_eq!(Shortcuts::default().repeat_time_ms, 500);
    }

    #[test]
    fn shortcuts_parses_repeat_time_ms() {
        let s: Shortcuts = toml::from_str("repeat-time-ms = 250\n").unwrap();
        assert_eq!(s.repeat_time_ms, 250);
    }

    #[test]
    fn normalize_keeps_zero_repeat_time() {
        let mut s = Shortcuts {
            repeat_time_ms: 0,
            ..Default::default()
        };
        s.normalize();
        assert_eq!(
            s.repeat_time_ms, 0,
            "0 means repeat disabled; it must survive normalize()"
        );
    }
}
