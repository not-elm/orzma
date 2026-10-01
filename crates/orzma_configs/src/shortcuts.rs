//! The `[shortcuts]` section: the chord grammar and the actions it binds.

use crate::error::{KeyChordParseError, OrzmaConfigsError, OrzmaConfigsResult};
use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Logical key: a single character or one of the named keys below.
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
    /// The literal `+` key, written `Plus` in a chord string (`"Cmd+Plus"`).
    Plus,
    /// An unrecognized logical key name, kept verbatim.
    Other(String),
}

impl Key {
    /// True when this logical key resolves to a physical `KeyCode` at runtime,
    /// so a leader bound to it can actually fire.
    ///
    /// # Invariants
    ///
    /// The accepted domain is exactly the keys that map to a physical
    /// `KeyCode`: an ASCII-alphanumeric `Char`, `Char('[')`, `Char(']')`,
    /// `Char('-')`, `Char('=')`, `Plus`, and every named key below. `Other`
    /// and any other character do not map.
    pub fn maps_to_physical_key(&self) -> bool {
        // NOTE: keep this domain in lockstep with `key_to_keycode`
        // (src/input/shortcuts.rs); a divergence silently disables the prefix
        // table (see the invariant above).
        match self {
            Key::Char(c) => c.is_ascii_alphanumeric() || matches!(c, '[' | ']' | '-' | '='),
            Key::Escape
            | Key::Space
            | Key::Enter
            | Key::Tab
            | Key::Backspace
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::Plus => true,
            Key::Other(_) => false,
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

/// Parses `"Cmd+Shift+S"`-shape strings into a `KeyChord`.
///
/// Modifier names are case-insensitive. Aliases: `Cmd` / `Command` / `Meta` /
/// `Super` all set `meta`; `Alt` / `Opt` / `Option` all set `alt`. ASCII letter
/// keys are normalized to lowercase (Shift is held in `Modifiers`, never in
/// key case). An empty string is not accepted.
///
/// # Errors
///
/// Returns [`OrzmaConfigsError::KeyChord`] when a token is empty or names no
/// known modifier or key, a modifier repeats, or more than one key appears.
pub fn parse_key_chord(s: &str) -> OrzmaConfigsResult<KeyChord> {
    if s.is_empty() {
        return Err(KeyChordParseError::EmptyToken.into());
    }
    let tokens: Vec<&str> = s.split('+').collect();
    if tokens.iter().any(|t| t.is_empty()) {
        return Err(KeyChordParseError::EmptyToken.into());
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
                }
                .into());
            }
            mods.meta = mods.meta || bit.meta;
            mods.ctrl = mods.ctrl || bit.ctrl;
            mods.alt = mods.alt || bit.alt;
            mods.shift = mods.shift || bit.shift;
        } else {
            if key.is_some() {
                return Err(KeyChordParseError::MultipleKeyTokens.into());
            }
            let k = Key::from_token(token);
            if let Key::Other(name) = &k {
                return Err(KeyChordParseError::UnknownNamedKey(name.clone()).into());
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

/// One chord-collision entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateChord {
    /// The chord that has multiple bindings.
    pub chord: KeyChord,
    /// Action labels (kebab-case TOML keys) that share this chord. Length >= 2.
    pub actions: Vec<&'static str>,
}

/// A bare modifier that can act as a tap leader. `Shift` is not accepted.
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
/// reached after the configured `leader`. In a config value, a leading `r:`
/// sets `repeat`, and a `<Leader>` token selects the `Leader` variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Fires when the chord is pressed directly.
    Direct {
        /// The chord to press.
        chord: KeyChord,
        /// True when bound with the `r:` token: the action fires again on
        /// each OS key repeat while the chord is held. Otherwise it fires once
        /// per press.
        repeat: bool,
    },
    /// Fires when the chord is pressed after the leader (`<Leader>x`).
    Leader {
        /// The second-key chord pressed after the leader.
        chord: KeyChord,
        /// True when bound with the `r:` token: after firing, the key
        /// re-fires within `repeat-time-ms` without re-pressing the leader.
        repeat: bool,
    },
}

impl Binding {
    /// The token that marks a binding value as leader-scoped (`<Leader>x`).
    /// Matched case-insensitively, after any `r:`, only.
    const LEADER_TOKEN: &'static str = "<Leader>";

    /// The token that marks a binding value as repeatable (`r:x`). Matched
    /// case-insensitively at the start of the value only.
    const REPEAT_TOKEN: &'static str = "r:";

    /// The chord to match: the direct chord, or the second-key chord for a
    /// leader-scoped binding.
    pub fn chord(&self) -> &KeyChord {
        match self {
            Binding::Direct { chord, .. } | Binding::Leader { chord, .. } => chord,
        }
    }

    /// Whether the binding was written with the `r:` token.
    pub fn repeat(&self) -> bool {
        match self {
            Binding::Direct { repeat, .. } | Binding::Leader { repeat, .. } => *repeat,
        }
    }

    /// Parses a non-empty config value: a leading `r:` sets `repeat`, then a
    /// leading `<Leader>` selects `Leader`; otherwise the rest parses as
    /// `Direct`. Both tokens match case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmaConfigsError::KeyChord`] when the chord after the
    /// tokens does not parse, including an empty one.
    fn parse(value: &str) -> OrzmaConfigsResult<Self> {
        let (rest, repeat) = match strip_token(value, Self::REPEAT_TOKEN) {
            Some(rest) => (rest, true),
            None => (value, false),
        };
        Ok(match strip_token(rest, Self::LEADER_TOKEN) {
            Some(chord) => Self::Leader {
                chord: parse_key_chord(chord)?,
                repeat,
            },
            None => Self::Direct {
                chord: parse_key_chord(rest)?,
                repeat,
            },
        })
    }
}

/// User-facing shortcut configuration: the leader chord plus one flat binding
/// per action. Each value is a chord string (`"Cmd+V"`), a leader-scoped chord
/// (`"<Leader>s"`), either one preceded by `r:` to make it repeatable, or `""`
/// (unbind). An omitted action keeps its default, and an unknown key is
/// rejected at load time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Shortcuts {
    /// The leader for `<Leader>`-scoped bindings: a chord (`Ctrl+A`) or a bare
    /// modifier tap (`Cmd`). An empty or absent value disables it.
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
    /// Step the terminal font size up.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub increase_font_size: Option<Binding>,
    /// Step the terminal font size down.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub decrease_font_size: Option<Binding>,
    /// Return the terminal font size to the configured `[font] size`.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub reset_font_size: Option<Binding>,
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
    /// Enter vi mode on the focused terminal.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub enter_vi_mode: Option<Binding>,
    /// Focus the pane to the left.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_left_pane: Option<Binding>,
    /// Focus the pane below.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_down_pane: Option<Binding>,
    /// Focus the pane above.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_up_pane: Option<Binding>,
    /// Focus the pane to the right.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_right_pane: Option<Binding>,
    /// Split the active pane side-by-side — vertical divider.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub split_vertical_pane: Option<Binding>,
    /// Split the active pane stacked — horizontal divider.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub split_horizontal_pane: Option<Binding>,
    /// Kill the active pane.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub kill_pane: Option<Binding>,
    /// Moves a divider of the active pane 5 cells left; repeatable.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_left_pane: Option<Binding>,
    /// Moves a divider of the active pane 5 cells down; repeatable.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_down_pane: Option<Binding>,
    /// Moves a divider of the active pane 5 cells up; repeatable.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_up_pane: Option<Binding>,
    /// Moves a divider of the active pane 5 cells right; repeatable.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub resize_right_pane: Option<Binding>,
    /// Open a new tab after the last one and display it.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub new_tab: Option<Binding>,
    /// Close the displayed tab and every pane in it.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub close_tab: Option<Binding>,
    /// Display the tab to the right, wrapping to the first.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub next_tab: Option<Binding>,
    /// Display the tab to the left, wrapping to the last.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub previous_tab: Option<Binding>,
    /// Display the first tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_1: Option<Binding>,
    /// Display the second tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_2: Option<Binding>,
    /// Display the third tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_3: Option<Binding>,
    /// Display the fourth tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_4: Option<Binding>,
    /// Display the fifth tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_5: Option<Binding>,
    /// Display the sixth tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_6: Option<Binding>,
    /// Display the seventh tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_7: Option<Binding>,
    /// Display the eighth tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_8: Option<Binding>,
    /// Display the ninth tab.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub select_tab_9: Option<Binding>,
    /// Rename the displayed tab in place.
    #[serde(
        deserialize_with = "deser_binding_or_unbind",
        serialize_with = "ser_binding_or_unbind"
    )]
    pub rename_tab: Option<Binding>,
    /// Timeout (ms) for a modifier-tap leader: press+release within this window
    /// with no intervening key/mouse press counts as a tap. Default 300; 0 is
    /// normalized to 300.
    pub leader_tap_timeout_ms: u64,
    /// Repeat window (ms) for `r:<Leader>` bindings: after such a binding
    /// fires, pressing a repeat-marked key again within this window re-fires
    /// it without the leader; each fire re-arms the window. Default 500.
    ///
    /// 0 disables repeat entirely and is not normalized away.
    pub repeat_time_ms: u64,
    /// Whether direct-chord bindings fire while a webview holds keyboard
    /// focus, instead of reaching the page. Default `true`.
    ///
    /// Direct `copy` and `paste` chords never fire while a webview has focus,
    /// and `<Leader>` bindings and `release-webview-focus` always do.
    pub direct_chords_over_webview: bool,
}

impl Default for Shortcuts {
    fn default() -> Self {
        let host = PlatformDefaults::for_host();
        Shortcuts {
            leader: host.leader,
            paste: host.paste,
            copy: host.copy,
            increase_font_size: host.increase_font_size,
            decrease_font_size: host.decrease_font_size,
            reset_font_size: host.reset_font_size,
            release_webview_focus: Some(parse_default_binding("<Leader>u")),
            quit: host.quit,
            enter_vi_mode: Some(parse_default_binding("Alt+s")),
            select_left_pane: Some(parse_default_binding("Alt+h")),
            select_down_pane: Some(parse_default_binding("Alt+j")),
            select_up_pane: Some(parse_default_binding("Alt+k")),
            select_right_pane: Some(parse_default_binding("Alt+l")),
            split_vertical_pane: Some(parse_default_binding("Alt+i")),
            split_horizontal_pane: Some(parse_default_binding("Alt+o")),
            kill_pane: Some(parse_default_binding("Alt+p")),
            resize_left_pane: Some(parse_default_binding("r:Alt+Shift+H")),
            resize_down_pane: Some(parse_default_binding("r:Alt+Shift+J")),
            resize_up_pane: Some(parse_default_binding("r:Alt+Shift+K")),
            resize_right_pane: Some(parse_default_binding("r:Alt+Shift+L")),
            new_tab: Some(parse_default_binding("Alt+c")),
            close_tab: Some(parse_default_binding("Alt+Shift+X")),
            next_tab: Some(parse_default_binding("Alt+]")),
            previous_tab: Some(parse_default_binding("Alt+[")),
            select_tab_1: Some(parse_default_binding("Alt+1")),
            select_tab_2: Some(parse_default_binding("Alt+2")),
            select_tab_3: Some(parse_default_binding("Alt+3")),
            select_tab_4: Some(parse_default_binding("Alt+4")),
            select_tab_5: Some(parse_default_binding("Alt+5")),
            select_tab_6: Some(parse_default_binding("Alt+6")),
            select_tab_7: Some(parse_default_binding("Alt+7")),
            select_tab_8: Some(parse_default_binding("Alt+8")),
            select_tab_9: Some(parse_default_binding("Alt+9")),
            rename_tab: Some(parse_default_binding("Alt+r")),
            leader_tap_timeout_ms: 300,
            repeat_time_ms: 500,
            direct_chords_over_webview: true,
        }
    }
}

impl Shortcuts {
    /// `(label, &Option<Binding>, action)` for every action, in stable order.
    pub fn bindings_iter(
        &self,
    ) -> impl Iterator<Item = (&'static str, &Option<Binding>, Shortcut)> + '_ {
        [
            ("paste", &self.paste, Shortcut::Paste),
            ("copy", &self.copy, Shortcut::Copy),
            (
                "increase-font-size",
                &self.increase_font_size,
                Shortcut::FontSize(FontSizeStep::Increase),
            ),
            (
                "decrease-font-size",
                &self.decrease_font_size,
                Shortcut::FontSize(FontSizeStep::Decrease),
            ),
            (
                "reset-font-size",
                &self.reset_font_size,
                Shortcut::FontSize(FontSizeStep::Reset),
            ),
            (
                "release-webview-focus",
                &self.release_webview_focus,
                Shortcut::ReleaseWebviewFocus,
            ),
            ("quit", &self.quit, Shortcut::Quit),
            ("enter-vi-mode", &self.enter_vi_mode, Shortcut::EnterViMode),
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
            ("new-tab", &self.new_tab, Shortcut::NewTab),
            ("close-tab", &self.close_tab, Shortcut::CloseTab),
            ("next-tab", &self.next_tab, Shortcut::NextTab),
            ("previous-tab", &self.previous_tab, Shortcut::PreviousTab),
            ("select-tab-1", &self.select_tab_1, Shortcut::SelectTab(1)),
            ("select-tab-2", &self.select_tab_2, Shortcut::SelectTab(2)),
            ("select-tab-3", &self.select_tab_3, Shortcut::SelectTab(3)),
            ("select-tab-4", &self.select_tab_4, Shortcut::SelectTab(4)),
            ("select-tab-5", &self.select_tab_5, Shortcut::SelectTab(5)),
            ("select-tab-6", &self.select_tab_6, Shortcut::SelectTab(6)),
            ("select-tab-7", &self.select_tab_7, Shortcut::SelectTab(7)),
            ("select-tab-8", &self.select_tab_8, Shortcut::SelectTab(8)),
            ("select-tab-9", &self.select_tab_9, Shortcut::SelectTab(9)),
            ("rename-tab", &self.rename_tab, Shortcut::RenameTab),
        ]
        .into_iter()
    }

    /// Bound direct chords only: `(label, chord, action, repeat)`.
    pub fn direct_chords(
        &self,
    ) -> impl Iterator<Item = (&'static str, &KeyChord, Shortcut, bool)> + '_ {
        self.bindings_iter()
            .filter_map(|(label, bound, action)| match bound {
                Some(Binding::Direct { chord, repeat }) => Some((label, chord, action, *repeat)),
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

    /// Detects chord collisions among direct bindings, reporting every one as
    /// [`OrzmaConfigsError::DuplicateChords`].
    pub(crate) fn validate_no_direct_conflicts(&self) -> OrzmaConfigsResult {
        conflicts(self.direct_chords()).map_err(OrzmaConfigsError::DuplicateChords)
    }

    /// Detects chord collisions among leader-scoped bindings, reporting every
    /// one as [`OrzmaConfigsError::DuplicatePrefixChords`].
    pub(crate) fn validate_no_leader_conflicts(&self) -> OrzmaConfigsResult {
        conflicts(self.leader_chords()).map_err(OrzmaConfigsError::DuplicatePrefixChords)
    }

    /// Normalizes numeric fields: a `leader_tap_timeout_ms` of 0 reverts to the
    /// 300 default.
    pub(crate) fn normalize(&mut self) {
        if self.leader_tap_timeout_ms == 0 {
            self.leader_tap_timeout_ms = 300;
        }
    }
}

/// A direction for the `select-*-pane` and `resize-*-pane` shortcut
/// actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDirection {
    /// Toward the left edge of the window.
    Left,
    /// Toward the bottom edge of the window.
    Down,
    /// Toward the top edge of the window.
    Up,
    /// Toward the right edge of the window.
    Right,
}

/// Which way a split divides the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitOrientation {
    /// A vertical divider: panes end up side by side.
    Vertical,
    /// A horizontal divider: panes end up stacked.
    Horizontal,
}

/// Which way a `font-size` shortcut moves the terminal font size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSizeStep {
    /// Step to the next larger size.
    Increase,
    /// Step to the next smaller size.
    Decrease,
    /// Return to the configured `[font] size`.
    Reset,
}

/// Shortcut actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// Paste the system clipboard into the active terminal.
    Paste,
    /// Copies the focused terminal's selection to the system clipboard.
    Copy,
    /// Steps the terminal font size.
    FontSize(FontSizeStep),
    /// Releases keyboard focus from a focused webview back to the terminal.
    ReleaseWebviewFocus,
    /// Quits the orzma application.
    Quit,
    /// Enters vi mode on the focused terminal.
    EnterViMode,
    /// Focuses the neighbor pane in the given direction.
    SelectPane(PaneDirection),
    /// Splits the active pane.
    SplitPane(SplitOrientation),
    /// Kills the active pane.
    KillPane,
    /// Moves a divider of the active pane in the given direction.
    ResizePane(PaneDirection),
    /// Opens a new tab after the last one and displays it.
    NewTab,
    /// Closes the displayed tab.
    CloseTab,
    /// Displays the tab to the right, wrapping to the first.
    NextTab,
    /// Displays the tab to the left, wrapping to the last.
    PreviousTab,
    /// Displays the tab with this 1-based tab number.
    SelectTab(u8),
    /// Renames the displayed tab in place.
    RenameTab,
}

/// Strips a leading, case-insensitive `token`, returning the text after it,
/// or `None` when `value` does not start with `token`.
fn strip_token<'a>(value: &'a str, token: &str) -> Option<&'a str> {
    let (head, rest) = value.split_at_checked(token.len())?;
    head.eq_ignore_ascii_case(token).then_some(rest)
}

/// serde field deserializer for `Option<Binding>`: empty string is unbind
/// (`None`); any other string parses via `Binding::parse`.
fn deser_binding_or_unbind<'de, D>(d: D) -> Result<Option<Binding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Ok(None);
    }
    Binding::parse(&s).map(Some).map_err(DeError::custom)
}

/// serde field serializer for `Option<Binding>`: `None` → `""`; otherwise the
/// chord, preceded by `<Leader>` for a leader-scoped binding and by `r:` for a
/// repeatable one (`r:<Leader>D`, `r:Cmd+Plus`).
fn ser_binding_or_unbind<S>(value: &Option<Binding>, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let text = match value {
        None => String::new(),
        Some(binding) => {
            let repeat = if binding.repeat() {
                Binding::REPEAT_TOKEN
            } else {
                ""
            };
            let leader = match binding {
                Binding::Direct { .. } => "",
                Binding::Leader { .. } => Binding::LEADER_TOKEN,
            };
            format!("{repeat}{leader}{}", binding.chord())
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
fn parse_leader(value: &str) -> OrzmaConfigsResult<Leader> {
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

/// The host-dependent slice of the default table: the bindings whose stock
/// value differs between macOS and the Ctrl-based platforms.
struct PlatformDefaults {
    leader: Option<Leader>,
    paste: Option<Binding>,
    copy: Option<Binding>,
    quit: Option<Binding>,
    increase_font_size: Option<Binding>,
    decrease_font_size: Option<Binding>,
    reset_font_size: Option<Binding>,
}

impl PlatformDefaults {
    /// The quartet for the platform this build targets: `Cmd`-based on macOS,
    /// `Ctrl`-based with an `Alt` tap leader elsewhere. `quit` is unbound off
    /// macOS, where the window manager already closes the window.
    fn for_host() -> Self {
        if cfg!(target_os = "macos") {
            PlatformDefaults {
                leader: Some(Leader::ModifierTap(TapModifier::Meta)),
                paste: Some(parse_default_binding("Cmd+V")),
                copy: Some(parse_default_binding("Cmd+C")),
                quit: Some(parse_default_binding("Cmd+Q")),
                increase_font_size: Some(parse_default_binding("r:Cmd+Plus")),
                decrease_font_size: Some(parse_default_binding("r:Cmd+-")),
                reset_font_size: Some(parse_default_binding("Cmd+0")),
            }
        } else {
            PlatformDefaults {
                leader: Some(Leader::ModifierTap(TapModifier::Alt)),
                paste: Some(parse_default_binding("Ctrl+V")),
                copy: Some(parse_default_binding("Ctrl+C")),
                quit: None,
                increase_font_size: Some(parse_default_binding("r:Ctrl+Plus")),
                decrease_font_size: Some(parse_default_binding("r:Ctrl+-")),
                reset_font_size: Some(parse_default_binding("Ctrl+0")),
            }
        }
    }
}

fn parse_default_binding(s: &str) -> Binding {
    Binding::parse(s).unwrap_or_else(|e| panic!("invalid default binding {s:?}: {e}"))
}

/// Detects chord collisions across a table's bound entries. The returned `Vec`
/// is sorted by chord.
fn conflicts<'a>(
    entries: impl Iterator<Item = (&'static str, &'a KeyChord, Shortcut, bool)>,
) -> Result<(), Vec<DuplicateChord>> {
    let mut by_chord: BTreeMap<KeyChord, Vec<&'static str>> = BTreeMap::new();
    for (label, chord, _action, _repeat) in entries {
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

    /// Asserts that a key with no stable physical position reports no mapping,
    /// so the resolver drops the binding instead of resolving it to the wrong
    /// key.
    ///
    /// Case: a config binds an action to `F12` or to `.`, neither of which the
    /// keycode table covers.
    #[test]
    fn maps_to_physical_key_false_for_other_and_unmapped_punctuation() {
        assert!(!Key::Other("f12".into()).maps_to_physical_key());
        assert!(!Key::Char('.').maps_to_physical_key());
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

    /// Asserts that `strip_token` strips only a leading, case-insensitive
    /// token.
    ///
    /// Case: a config value puts `<Leader>` at the start, in the middle, or
    /// nowhere.
    #[test]
    fn strip_token_only_at_start() {
        let leader = Binding::LEADER_TOKEN;
        assert_eq!(strip_token("<Leader>s", leader), Some("s"));
        assert_eq!(strip_token("<leader>Ctrl+d", leader), Some("Ctrl+d"));
        assert_eq!(strip_token("Cmd+<Leader>", leader), None);
        assert_eq!(strip_token("s", leader), None);
        assert_eq!(strip_token("", leader), None);
    }

    /// Asserts that a value without a token parses as a single-fire direct
    /// binding and a `<Leader>` value as a single-fire leader binding.
    ///
    /// Case: a user binds paste to `Cmd+V`, vi mode to `<Leader>s`, and a pane
    /// action to `<Leader>Ctrl+d`.
    #[test]
    fn parse_binding_direct_and_leader() {
        assert_eq!(
            Binding::parse("Cmd+V").unwrap(),
            Binding::Direct {
                chord: parse_key_chord("Cmd+V").unwrap(),
                repeat: false,
            }
        );
        assert_eq!(
            Binding::parse("<Leader>s").unwrap(),
            Binding::Leader {
                chord: parse_key_chord("s").unwrap(),
                repeat: false,
            }
        );
        assert_eq!(
            Binding::parse("<Leader>Ctrl+d").unwrap(),
            Binding::Leader {
                chord: parse_key_chord("Ctrl+d").unwrap(),
                repeat: false,
            }
        );
    }

    /// Asserts that the `<Leader>` token matches in any letter case.
    ///
    /// Case: a user writes `<leader>s` or `<LEADER>s` in their config file.
    #[test]
    fn parse_binding_leader_token_case_insensitive() {
        let want = Binding::Leader {
            chord: parse_key_chord("s").unwrap(),
            repeat: false,
        };
        assert_eq!(Binding::parse("<leader>s").unwrap(), want);
        assert_eq!(Binding::parse("<LEADER>s").unwrap(), want);
    }

    /// Asserts that a `<Leader>` token with no chord after it is an error.
    ///
    /// Case: a user writes `kill-pane = "<Leader>"` and forgets the key.
    #[test]
    fn parse_binding_empty_after_leader_is_err() {
        assert!(Binding::parse("<Leader>").is_err());
    }

    /// Asserts that `Binding::chord` returns the chord without the leader
    /// token.
    ///
    /// Case: the resolver reads the key of a direct `Cmd+V` binding and of a
    /// `<Leader>s` binding.
    #[test]
    fn binding_chord_extracts_inner() {
        assert_eq!(
            Binding::parse("Cmd+V").unwrap().chord(),
            &parse_key_chord("Cmd+V").unwrap()
        );
        assert_eq!(
            Binding::parse("<Leader>s").unwrap().chord(),
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

    /// Asserts that the macOS default table binds the `Cmd` tap leader and 32
    /// direct chords, leaving only `release-webview-focus` leader-scoped.
    ///
    /// Case: a user on macOS starts orzma with no config file at all.
    #[cfg(target_os = "macos")]
    #[test]
    fn shortcuts_default_is_active_direct_bindings() {
        let s = Shortcuts::default();
        assert_eq!(s.leader, Some(Leader::ModifierTap(TapModifier::Meta)));
        assert_eq!(s.paste, Some(parse_default_binding("Cmd+V")));
        assert_eq!(s.quit, Some(parse_default_binding("Cmd+Q")));
        assert_eq!(s.copy, Some(parse_default_binding("Cmd+C")));
        assert_eq!(s.bindings_iter().count(), 33);
        assert_eq!(s.direct_chords().count(), 32);
        assert_eq!(s.leader_chords().count(), 1);
    }

    /// Asserts that the non-macOS default table binds the `Alt` tap leader and
    /// 31 direct chords, leaves only `release-webview-focus` leader-scoped, and
    /// leaves `quit` unbound rather than binding a chord the window manager
    /// already owns.
    ///
    /// Case: a user on Windows starts orzma with no config file at all, where
    /// no `Cmd` key exists to press.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn shortcuts_default_is_active_direct_bindings() {
        let s = Shortcuts::default();
        assert_eq!(s.leader, Some(Leader::ModifierTap(TapModifier::Alt)));
        assert_eq!(s.paste, Some(parse_default_binding("Ctrl+V")));
        assert_eq!(s.copy, Some(parse_default_binding("Ctrl+C")));
        assert_eq!(s.quit, None);
        assert_eq!(s.bindings_iter().count(), 33);
        assert_eq!(s.direct_chords().count(), 31);
        assert_eq!(s.leader_chords().count(), 1);
    }

    /// Asserts that every chord in the host default table is unique, so no
    /// stock binding shadows another.
    ///
    /// Case: a user runs orzma with no config file, on whichever platform the
    /// build targets.
    #[test]
    fn host_default_table_has_no_duplicate_chords() {
        let s = Shortcuts::default();
        assert!(s.validate_no_direct_conflicts().is_ok());
        assert!(s.validate_no_leader_conflicts().is_ok());
    }

    /// Asserts that every direct chord in the host default table resolves to a
    /// physical key, so a stock binding can actually fire.
    ///
    /// Case: a user presses a stock direct chord — `Ctrl+V` on Windows,
    /// `Cmd+V` on macOS — on a fresh install.
    #[test]
    fn host_default_direct_chords_map_to_physical_keys() {
        for (label, chord, _action, _repeat) in Shortcuts::default().direct_chords() {
            assert!(
                chord.key.maps_to_physical_key(),
                "default chord for {label:?} ({chord}) resolves to no physical key"
            );
        }
    }

    /// Asserts that `bindings_iter` lists one entry for each of the 33
    /// actions.
    ///
    /// Case: orzma starts and turns the configured actions into its shortcut
    /// table.
    #[test]
    fn bindings_iter_count_is_pinned_to_field_count() {
        assert_eq!(Shortcuts::default().bindings_iter().count(), 33);
    }

    /// Asserts that the stock resize bindings are repeatable `Alt+Shift`
    /// direct chords.
    ///
    /// Case: a user with no config file holds `Alt+Shift+H` to move a divider.
    #[test]
    fn default_resize_bindings_are_repeatable_alt_chords() {
        let s = Shortcuts::default();
        for (binding, chord) in [
            (&s.resize_left_pane, "r:Alt+Shift+H"),
            (&s.resize_down_pane, "r:Alt+Shift+J"),
            (&s.resize_up_pane, "r:Alt+Shift+K"),
            (&s.resize_right_pane, "r:Alt+Shift+L"),
        ] {
            assert_eq!(*binding, Some(parse_default_binding(chord)), "{chord}");
        }
    }

    /// Asserts that the stock pane actions are single-fire `Alt` direct
    /// chords.
    ///
    /// Case: a user with no config file splits, selects, and kills panes with
    /// `Alt` chords.
    #[test]
    fn default_multiplexer_actions_are_alt_chords() {
        let s = Shortcuts::default();
        for (binding, chord) in [
            (&s.select_left_pane, "Alt+h"),
            (&s.select_down_pane, "Alt+j"),
            (&s.select_up_pane, "Alt+k"),
            (&s.select_right_pane, "Alt+l"),
            (&s.split_vertical_pane, "Alt+i"),
            (&s.split_horizontal_pane, "Alt+o"),
            (&s.kill_pane, "Alt+p"),
            (&s.enter_vi_mode, "Alt+s"),
        ] {
            assert_eq!(*binding, Some(parse_default_binding(chord)), "{chord}");
        }
    }

    /// Asserts the stock tab bindings: `Alt` direct chords on every
    /// platform.
    ///
    /// Case: a new user opens a second tab, cycles, jumps to the third
    /// tab, and renames it without editing the config.
    #[test]
    fn tab_bindings_default_to_alt_chords() {
        let s = Shortcuts::default();
        let binding_of = |shortcut| {
            s.bindings_iter()
                .find(|(_, _, action)| *action == shortcut)
                .and_then(|(_, binding, _)| binding.clone())
        };
        for (shortcut, chord) in [
            (Shortcut::NewTab, "Alt+c"),
            (Shortcut::CloseTab, "Alt+Shift+X"),
            (Shortcut::NextTab, "Alt+]"),
            (Shortcut::PreviousTab, "Alt+["),
            (Shortcut::SelectTab(3), "Alt+3"),
            (Shortcut::RenameTab, "Alt+r"),
        ] {
            assert_eq!(
                binding_of(shortcut),
                Some(parse_default_binding(chord)),
                "{chord}"
            );
        }
    }

    /// Asserts that a pane action accepts a leader chord, an empty string that
    /// unbinds it, and a direct chord.
    ///
    /// Case: a user moves the vertical split to `<Leader>g`, turns off
    /// kill-pane, and puts the horizontal split on `Cmd+T`.
    #[test]
    fn multiplexer_actions_parse_from_flat_toml() {
        let toml = r#"
split-vertical-pane = "<Leader>g"
kill-pane = ""
split-horizontal-pane = "Cmd+T"
"#;
        let s: Shortcuts = toml::from_str(toml).unwrap();
        assert_eq!(
            s.split_vertical_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("g").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(s.kill_pane, None);
        assert_eq!(
            s.split_horizontal_pane,
            Some(Binding::Direct {
                chord: parse_key_chord("Cmd+T").unwrap(),
                repeat: false,
            })
        );
    }

    #[test]
    fn default_shortcuts_has_no_conflicts() {
        let s = Shortcuts::default();
        assert!(s.validate_no_direct_conflicts().is_ok());
        assert!(s.validate_no_leader_conflicts().is_ok());
    }

    /// Asserts that a chord leader and leader-scoped bindings parse from flat
    /// keys while the other actions keep their defaults.
    ///
    /// Case: a user sets a `Ctrl+A` leader and moves vi mode and kill-pane to
    /// new leader chords.
    #[test]
    fn shortcuts_parses_flat_leader_and_bindings() {
        let toml = r#"
leader = "Ctrl+A"
enter-vi-mode = "<Leader>s"
kill-pane = "<Leader>d"
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
            s.kill_pane,
            Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            })
        );
        assert_eq!(s.paste, Shortcuts::default().paste);
        assert_eq!(s.leader_chords().count(), 3);
    }

    #[test]
    fn shortcuts_rejects_unknown_field() {
        assert!(toml::from_str::<Shortcuts>("resize-pane-down = \"d\"\n").is_err());
    }

    /// Asserts that two direct bindings sharing one chord are reported as a
    /// single conflict naming both actions.
    ///
    /// Case: a user binds paste and quit to the same chord by hand.
    #[test]
    fn direct_conflict_detected() {
        let s = Shortcuts {
            paste: Some(Binding::Direct {
                chord: parse_key_chord("Ctrl+Alt+Q").unwrap(),
                repeat: false,
            }),
            quit: Some(Binding::Direct {
                chord: parse_key_chord("Ctrl+Alt+Q").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let Err(OrzmaConfigsError::DuplicateChords(err)) = s.validate_no_direct_conflicts() else {
            panic!("expected DuplicateChords");
        };
        assert_eq!(err.len(), 1);
        assert!(err[0].actions.contains(&"paste"));
        assert!(err[0].actions.contains(&"quit"));
    }

    /// Asserts that two leader-scoped bindings sharing one chord are reported
    /// as a single conflict naming both actions.
    ///
    /// Case: a user binds vi mode and kill-pane to `<Leader>d`.
    #[test]
    fn leader_conflict_detected() {
        let s = Shortcuts {
            enter_vi_mode: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            kill_pane: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let Err(OrzmaConfigsError::DuplicatePrefixChords(err)) = s.validate_no_leader_conflicts()
        else {
            panic!("expected DuplicatePrefixChords");
        };
        assert_eq!(err.len(), 1);
        assert!(err[0].actions.contains(&"enter-vi-mode"));
        assert!(err[0].actions.contains(&"kill-pane"));
    }

    /// Asserts that a repeatable and a non-repeatable leader binding on the
    /// same chord still conflict.
    ///
    /// Case: a user binds vi mode to `r:<Leader>d` and kill-pane to
    /// `<Leader>d`.
    #[test]
    fn leader_conflict_detected_across_repeat_flag() {
        let s = Shortcuts {
            enter_vi_mode: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: true,
            }),
            kill_pane: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: false,
            }),
            ..Default::default()
        };
        let Err(OrzmaConfigsError::DuplicatePrefixChords(err)) = s.validate_no_leader_conflicts()
        else {
            panic!("expected DuplicatePrefixChords");
        };
        assert_eq!(err.len(), 1);
        assert!(err[0].actions.contains(&"enter-vi-mode"));
        assert!(err[0].actions.contains(&"kill-pane"));
    }

    /// Asserts that the macOS default table round-trips to its exact JSON
    /// form, pinning every one of the 37 fields at once.
    ///
    /// Case: a macOS user's config is serialized back out.
    #[cfg(target_os = "macos")]
    #[test]
    fn default_shortcuts_json_snapshot() {
        let json = serde_json::to_string(&Shortcuts::default()).unwrap();
        let expected = r#"{"leader":"Cmd","paste":"Cmd+V","copy":"Cmd+C","increase-font-size":"r:Cmd+Plus","decrease-font-size":"r:Cmd+-","reset-font-size":"Cmd+0","release-webview-focus":"<Leader>U","quit":"Cmd+Q","enter-vi-mode":"Alt+S","select-left-pane":"Alt+H","select-down-pane":"Alt+J","select-up-pane":"Alt+K","select-right-pane":"Alt+L","split-vertical-pane":"Alt+I","split-horizontal-pane":"Alt+O","kill-pane":"Alt+P","resize-left-pane":"r:Alt+Shift+H","resize-down-pane":"r:Alt+Shift+J","resize-up-pane":"r:Alt+Shift+K","resize-right-pane":"r:Alt+Shift+L","new-tab":"Alt+C","close-tab":"Alt+Shift+X","next-tab":"Alt+]","previous-tab":"Alt+[","select-tab-1":"Alt+1","select-tab-2":"Alt+2","select-tab-3":"Alt+3","select-tab-4":"Alt+4","select-tab-5":"Alt+5","select-tab-6":"Alt+6","select-tab-7":"Alt+7","select-tab-8":"Alt+8","select-tab-9":"Alt+9","rename-tab":"Alt+R","leader-tap-timeout-ms":300,"repeat-time-ms":500,"direct-chords-over-webview":true}"#;
        assert_eq!(json, expected);
    }

    /// Asserts that the non-macOS default table round-trips to its exact JSON
    /// form, pinning every one of the 37 fields at once, with the unbound
    /// `quit` emitted as an empty string.
    ///
    /// Case: a Windows user's config is serialized back out.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn default_shortcuts_json_snapshot() {
        let json = serde_json::to_string(&Shortcuts::default()).unwrap();
        let expected = r#"{"leader":"Alt","paste":"Ctrl+V","copy":"Ctrl+C","increase-font-size":"r:Ctrl+Plus","decrease-font-size":"r:Ctrl+-","reset-font-size":"Ctrl+0","release-webview-focus":"<Leader>U","quit":"","enter-vi-mode":"Alt+S","select-left-pane":"Alt+H","select-down-pane":"Alt+J","select-up-pane":"Alt+K","select-right-pane":"Alt+L","split-vertical-pane":"Alt+I","split-horizontal-pane":"Alt+O","kill-pane":"Alt+P","resize-left-pane":"r:Alt+Shift+H","resize-down-pane":"r:Alt+Shift+J","resize-up-pane":"r:Alt+Shift+K","resize-right-pane":"r:Alt+Shift+L","new-tab":"Alt+C","close-tab":"Alt+Shift+X","next-tab":"Alt+]","previous-tab":"Alt+[","select-tab-1":"Alt+1","select-tab-2":"Alt+2","select-tab-3":"Alt+3","select-tab-4":"Alt+4","select-tab-5":"Alt+5","select-tab-6":"Alt+6","select-tab-7":"Alt+7","select-tab-8":"Alt+8","select-tab-9":"Alt+9","rename-tab":"Alt+R","leader-tap-timeout-ms":300,"repeat-time-ms":500,"direct-chords-over-webview":true}"#;
        assert_eq!(json, expected);
    }

    /// Asserts that a chord leader serializes as its chord string and a
    /// leader-scoped binding with the `<Leader>` token.
    ///
    /// Case: a user with a `Ctrl+A` leader and kill-pane on `<Leader>d` has
    /// their config serialized back out.
    #[test]
    fn serialize_leader_binding_emits_leader_token() {
        let s = Shortcuts {
            leader: Some(Leader::Chord(parse_key_chord("Ctrl+A").unwrap())),
            kill_pane: Some(Binding::Leader {
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
            json.contains(r#""kill-pane":"<Leader>D""#),
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

    /// Asserts that a bare-modifier leader parses as a tap leader and keeps
    /// the configured tap timeout.
    ///
    /// Case: a user makes a tap of `Cmd` the leader with a 250 ms tap window.
    #[test]
    fn shortcuts_parses_bare_modifier_leader_and_timeout() {
        let toml = "leader = \"Cmd\"\nleader-tap-timeout-ms = 250\nkill-pane = \"<Leader>d\"\n";
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

    /// Asserts that a leading `r:`, in either case, marks a leader or a direct
    /// binding as repeatable.
    ///
    /// Case: a user writes `r:<Leader>h`, `R:<LEADER>h`, and `r:Alt+Shift+H`
    /// in their config file.
    #[test]
    fn parse_binding_reads_the_repeat_token() {
        let leader = Binding::Leader {
            chord: parse_key_chord("h").unwrap(),
            repeat: true,
        };
        assert_eq!(Binding::parse("r:<Leader>h").unwrap(), leader);
        assert_eq!(Binding::parse("R:<LEADER>h").unwrap(), leader);
        assert_eq!(
            Binding::parse("r:Alt+Shift+H").unwrap(),
            Binding::Direct {
                chord: parse_key_chord("Alt+Shift+H").unwrap(),
                repeat: true,
            }
        );
    }

    /// Asserts that `r:` with nothing to repeat is an error.
    ///
    /// Case: a user leaves the chord out after the token, or doubles the
    /// token.
    #[test]
    fn parse_binding_rejects_an_empty_repeat_binding() {
        assert!(Binding::parse("r:").is_err());
        assert!(Binding::parse("r:<Leader>").is_err());
        assert!(Binding::parse("r:r:x").is_err());
    }

    /// Asserts that the old `<Leader:r>` spelling is rejected, with or without
    /// a leading `r:`.
    ///
    /// Case: a user upgrades with a config that still says
    /// `resize-left-pane = "<Leader:r>Shift+H"`.
    #[test]
    fn parse_binding_rejects_the_leader_repeat_token() {
        for value in ["<Leader:r>h", "<leader:R>h", "r:<Leader:r>h"] {
            assert!(Binding::parse(value).is_err(), "{value}");
        }
    }

    /// Asserts that repeatable bindings serialize with the `r:` token.
    ///
    /// Case: a config with kill-pane on `r:<Leader>d` and zoom-in on
    /// `r:Cmd+Plus` is serialized back out.
    #[test]
    fn serialize_repeat_bindings_emit_the_repeat_token() {
        let s = Shortcuts {
            kill_pane: Some(Binding::Leader {
                chord: parse_key_chord("d").unwrap(),
                repeat: true,
            }),
            increase_font_size: Some(Binding::Direct {
                chord: parse_key_chord("Cmd+Plus").unwrap(),
                repeat: true,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""kill-pane":"r:<Leader>D""#), "{json}");
        assert!(
            json.contains(r#""increase-font-size":"r:Cmd+Plus""#),
            "{json}"
        );
    }

    /// Asserts that each binding form parses back to itself after
    /// serialization.
    ///
    /// Case: orzma writes a config it loaded and reads it again.
    #[test]
    fn repeat_bindings_round_trip() {
        for value in ["r:<Leader>h", "<Leader>h", "r:Alt+Shift+H", "Alt+H"] {
            let parsed = Some(Binding::parse(value).unwrap());
            let s = Shortcuts {
                kill_pane: parsed.clone(),
                ..Default::default()
            };
            let json = serde_json::to_string(&s).unwrap();
            let back: Shortcuts = serde_json::from_str(&json).unwrap();
            assert_eq!(back.kill_pane, parsed, "{value}");
        }
    }

    /// Asserts that `leader_chords` reports each leader binding's repeat
    /// flag.
    ///
    /// Case: a user makes vi mode repeatable on `r:<Leader>s` and leaves
    /// kill-pane non-repeatable on `<Leader>d`.
    #[test]
    fn leader_chords_carries_repeat_flag() {
        let s = Shortcuts {
            enter_vi_mode: Some(Binding::Leader {
                chord: parse_key_chord("s").unwrap(),
                repeat: true,
            }),
            kill_pane: Some(Binding::Leader {
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
        assert!(entries.iter().any(|(l, _, _, r)| *l == "kill-pane" && !*r));
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

    /// Asserts that direct chords take priority over a focused webview by
    /// default.
    ///
    /// Case: a user with no `[shortcuts]` table clicks into a webview and
    /// presses `Cmd+Plus` to zoom the terminal font.
    #[test]
    fn shortcuts_default_direct_chords_over_webview_is_true() {
        assert!(Shortcuts::default().direct_chords_over_webview);
    }

    /// Asserts that `direct-chords-over-webview = false` turns the priority
    /// off.
    ///
    /// Case: a user whose page handles `Cmd+Plus` itself sets the key to
    /// `false` so the page keeps that chord while focused.
    #[test]
    fn shortcuts_parses_direct_chords_over_webview() {
        let s: Shortcuts = toml::from_str("direct-chords-over-webview = false\n").unwrap();
        assert!(!s.direct_chords_over_webview);
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

    /// Asserts that the keys the zoom bindings use report a physical mapping,
    /// so the config layer does not warn them away as unreachable.
    ///
    /// Case: the shipped platform defaults bind `Cmd+Plus` / `Cmd+-` /
    /// `Cmd+0` and must survive the resolution pass.
    #[test]
    fn zoom_keys_map_to_physical_keys() {
        assert!(Key::Plus.maps_to_physical_key());
        assert!(Key::Char('-').maps_to_physical_key());
        assert!(Key::Char('=').maps_to_physical_key());
    }

    /// Asserts that the three zoom actions ship with platform-appropriate
    /// direct bindings.
    ///
    /// Case: a user installs orzma with no config file and presses the zoom
    /// keys.
    #[test]
    fn zoom_actions_have_platform_defaults() {
        let sc = Shortcuts::default();
        let modifier = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };

        assert_eq!(
            sc.increase_font_size,
            Some(parse_default_binding(&format!("r:{modifier}+Plus")))
        );
        assert_eq!(
            sc.decrease_font_size,
            Some(parse_default_binding(&format!("r:{modifier}+-")))
        );
        assert_eq!(
            sc.reset_font_size,
            Some(parse_default_binding(&format!("{modifier}+0")))
        );
    }

    /// Asserts that each zoom action reaches `bindings_iter` under its
    /// kebab-case config key.
    ///
    /// Case: a user rebinds `increase-font-size` in their config file.
    #[test]
    fn zoom_actions_appear_in_bindings_iter() {
        let sc = Shortcuts::default();
        let labels: Vec<&'static str> = sc.bindings_iter().map(|(label, _, _)| label).collect();

        assert!(labels.contains(&"increase-font-size"));
        assert!(labels.contains(&"decrease-font-size"));
        assert!(labels.contains(&"reset-font-size"));
    }
}
