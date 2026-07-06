//! `[vi-mode]` table: shared vi-mode key bindings for both Default and
//! tmux modes. Keys are LOGICAL (case-sensitive characters, symbols allowed)
//! with an optional `Ctrl+` prefix — a different grammar from `[shortcuts]`
//! chords, matching how vi-mode keys are actually decided at runtime.

use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Named (non-character) keys accepted in `[vi-mode]` bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViModeNamedKey {
    /// `Escape`.
    Escape,
    /// `Enter`.
    Enter,
    /// `Space`.
    Space,
    /// `Tab`.
    Tab,
    /// `Backspace`.
    Backspace,
    /// `ArrowUp`.
    ArrowUp,
    /// `ArrowDown`.
    ArrowDown,
    /// `ArrowLeft`.
    ArrowLeft,
    /// `ArrowRight`.
    ArrowRight,
}

impl ViModeNamedKey {
    fn from_token(s: &str) -> Option<Self> {
        match s {
            "Escape" => Some(Self::Escape),
            "Enter" => Some(Self::Enter),
            "Space" => Some(Self::Space),
            "Tab" => Some(Self::Tab),
            "Backspace" => Some(Self::Backspace),
            "ArrowUp" => Some(Self::ArrowUp),
            "ArrowDown" => Some(Self::ArrowDown),
            "ArrowLeft" => Some(Self::ArrowLeft),
            "ArrowRight" => Some(Self::ArrowRight),
            _ => None,
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Escape => "Escape",
            Self::Enter => "Enter",
            Self::Space => "Space",
            Self::Tab => "Tab",
            Self::Backspace => "Backspace",
            Self::ArrowUp => "ArrowUp",
            Self::ArrowDown => "ArrowDown",
            Self::ArrowLeft => "ArrowLeft",
            Self::ArrowRight => "ArrowRight",
        }
    }
}

/// The key part of a `[vi-mode]` binding: an exact logical character
/// (case-sensitive — `"V"` means Shift+v as typed) or a named key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViModeBaseKey {
    /// A single logical character, matched exactly as typed.
    Char(String),
    /// A named key.
    Named(ViModeNamedKey),
}

/// One `[vi-mode]` binding entry: an optional `Ctrl+` prefix plus a key.
///
/// # Invariants
///
/// When `ctrl` is true the key MUST be `Char` of one ASCII-alphanumeric
/// character (stored lowercase) or `Named` — `Ctrl+` entries are matched on
/// the physical `KeyCode` at runtime, which only exists for that domain.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViModeKey {
    /// `Ctrl` is part of the binding.
    pub ctrl: bool,
    /// The key to match.
    pub key: ViModeBaseKey,
}

impl fmt::Display for ViModeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        match &self.key {
            ViModeBaseKey::Char(c) => write!(f, "{c}"),
            ViModeBaseKey::Named(n) => write!(f, "{}", n.token()),
        }
    }
}

/// Reason a vi-mode key string failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViModeKeyParseError {
    /// Empty key token.
    #[error("empty vi-mode key")]
    Empty,
    /// A modifier other than `Ctrl` (`Cmd`/`Alt`/`Shift`/aliases).
    #[error(
        "modifier {0:?} is not allowed in [vi-mode] (only Ctrl+); express Shift via the character case"
    )]
    ForbiddenModifier(String),
    /// More than one `+`-separated segment beyond `Ctrl+<key>`.
    #[error("too many tokens in vi-mode key {0:?} (expected [Ctrl+]<key>)")]
    TooManyTokens(String),
    /// A multi-character token that is not a known named key.
    #[error("unknown vi-mode key {0:?} (expected one character or a named key)")]
    UnknownKey(String),
    /// `Ctrl+` with a non-alphanumeric character.
    #[error("Ctrl+{0:?} is not allowed (Ctrl accepts ASCII alphanumerics and named keys only)")]
    CtrlNonAlphanumeric(String),
}

/// Parses one `[vi-mode]` key string (`"V"`, `"$"`, `"Ctrl+F"`, `"Escape"`).
pub fn parse_vi_mode_key(s: &str) -> Result<ViModeKey, ViModeKeyParseError> {
    if s.is_empty() {
        return Err(ViModeKeyParseError::Empty);
    }
    // NOTE: a bare "+" would split into two empty tokens and be misread as a
    // forbidden modifier — and a config parse error discards the user's WHOLE
    // config (warn + defaults), so "+" must short-circuit before the split.
    if s == "+" {
        return Ok(ViModeKey {
            ctrl: false,
            key: ViModeBaseKey::Char("+".to_string()),
        });
    }
    let parts: Vec<&str> = s.split('+').collect();
    let (ctrl, key_token) = match parts.as_slice() {
        [key] => (false, *key),
        [modifier, key] if modifier.eq_ignore_ascii_case("ctrl") => (true, *key),
        [modifier, _] => {
            return Err(ViModeKeyParseError::ForbiddenModifier(
                (*modifier).to_string(),
            ));
        }
        _ => return Err(ViModeKeyParseError::TooManyTokens(s.to_string())),
    };
    if key_token.is_empty() {
        return Err(ViModeKeyParseError::Empty);
    }
    if let Some(named) = ViModeNamedKey::from_token(key_token) {
        return Ok(ViModeKey {
            ctrl,
            key: ViModeBaseKey::Named(named),
        });
    }
    let mut chars = key_token.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(ViModeKeyParseError::UnknownKey(key_token.to_string()));
    };
    if ctrl {
        if !c.is_ascii_alphanumeric() {
            return Err(ViModeKeyParseError::CtrlNonAlphanumeric(
                key_token.to_string(),
            ));
        }
        return Ok(ViModeKey {
            ctrl: true,
            key: ViModeBaseKey::Char(c.to_ascii_lowercase().to_string()),
        });
    }
    // NOTE: the space BAR arrives as the named logical key, never as the
    // character " " — a Char(" ") entry would parse fine yet be forever dead,
    // so normalize it to the named form the resolver actually matches.
    if c == ' ' {
        return Ok(ViModeKey {
            ctrl: false,
            key: ViModeBaseKey::Named(ViModeNamedKey::Space),
        });
    }
    Ok(ViModeKey {
        ctrl: false,
        key: ViModeBaseKey::Char(c.to_string()),
    })
}

/// A cursor motion, in mode-neutral vocabulary. The binary maps this to the
/// engine's `ViMotion` (Default mode) or a tmux `-X` command (tmux mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViModeMotion {
    /// One cell left.
    Left,
    /// One cell down.
    Down,
    /// One cell up.
    Up,
    /// One cell right.
    Right,
    /// Column 0.
    LineStart,
    /// Last column.
    LineEnd,
    /// First non-blank column.
    LineFirstChar,
    /// Next word start (semantic).
    NextWord,
    /// Previous word start (semantic).
    PreviousWord,
    /// Next word end (semantic).
    NextWordEnd,
    /// Next space-delimited word start.
    NextSpace,
    /// Previous space-delimited word start.
    PreviousSpace,
    /// Next space-delimited word end.
    NextSpaceEnd,
    /// Top visible line.
    ScreenTop,
    /// Middle visible line.
    ScreenMiddle,
    /// Bottom visible line.
    ScreenBottom,
    /// Previous paragraph boundary.
    PreviousParagraph,
    /// Next paragraph boundary.
    NextParagraph,
    /// Matching bracket.
    MatchingBracket,
}

/// A viewport scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViModeScroll {
    /// Oldest history line.
    HistoryTop,
    /// Newest line (live tail).
    HistoryBottom,
    /// One page up.
    PageUp,
    /// One page down.
    PageDown,
    /// Half a page up.
    HalfPageUp,
    /// Half a page down.
    HalfPageDown,
    /// One line up.
    ScrollUp,
    /// One line down.
    ScrollDown,
}

/// A selection kind to toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViModeSelection {
    /// Character-wise selection.
    Simple,
    /// Line-wise selection.
    Lines,
    /// Rectangular (block) selection.
    Rect,
}

/// A prompt-opening action (search regex or jump char).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViModePromptDir {
    /// `/` — search down.
    SearchForward,
    /// `?` — search up.
    SearchBackward,
    /// `f` — jump to char forward.
    JumpForward,
    /// `F` — jump to char backward.
    JumpBackward,
    /// `t` — jump till char forward.
    JumpToForward,
    /// `T` — jump till char backward.
    JumpToBackward,
}

/// A repeat of the previous search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViModeSearchStep {
    /// Repeat in the search direction (`n`).
    Next,
    /// Repeat against the search direction (`N`).
    Previous,
}

/// One vi-mode action, in config-crate (engine-free) vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViModeAction {
    /// Move the copy cursor.
    Motion(ViModeMotion),
    /// Scroll the viewport.
    Scroll(ViModeScroll),
    /// Toggle a selection of the given kind.
    Selection(ViModeSelection),
    /// Copy the selection and leave vi mode.
    Yank,
    /// Leave vi mode.
    Exit,
    /// Open a search / jump prompt (tmux mode only for now).
    Prompt(ViModePromptDir),
    /// Repeat the previous search (tmux mode only for now).
    SearchStep(ViModeSearchStep),
}

/// One key with multiple bindings. Carried inside
/// `OrzmaConfigsError::DuplicateViModeKeys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateViModeKey {
    /// The key bound more than once.
    pub key: ViModeKey,
    /// Action labels (kebab-case TOML keys) sharing the key. Length >= 2.
    pub actions: Vec<&'static str>,
}

macro_rules! vi_mode_fields {
    ($($(#[$doc:meta])* $field:ident => $label:literal, $action:expr, [$($default:literal),*];)+) => {
        /// User-facing `[vi-mode]` configuration: one flat table of vi-mode
        /// actions, each bound to zero or more keys (string or array of strings
        /// in TOML; `""` / `[]` unbinds).
        #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
        #[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
        pub struct ViModeConfig {
            $(
                $(#[$doc])*
                #[serde(
                    deserialize_with = "deser_keys",
                    serialize_with = "ser_keys"
                )]
                pub $field: Vec<ViModeKey>,
            )+
        }

        impl Default for ViModeConfig {
            fn default() -> Self {
                ViModeConfig {
                    $($field: vec![$(parse_default_key($default)),*],)+
                }
            }
        }

        impl ViModeConfig {
            /// `(label, &keys, action)` for every action, in stable order.
            /// The single source of truth for the `[vi-mode]` schema.
            pub fn bindings_iter(
                &self,
            ) -> impl Iterator<Item = (&'static str, &Vec<ViModeKey>, ViModeAction)> + '_ {
                [
                    $(($label, &self.$field, $action),)+
                ]
                .into_iter()
            }
        }
    };
}

vi_mode_fields! {
    /// Move the cursor one cell left.
    cursor_left => "cursor-left", ViModeAction::Motion(ViModeMotion::Left), ["h", "ArrowLeft"];
    /// Move the cursor one cell down.
    cursor_down => "cursor-down", ViModeAction::Motion(ViModeMotion::Down), ["j", "ArrowDown"];
    /// Move the cursor one cell up.
    cursor_up => "cursor-up", ViModeAction::Motion(ViModeMotion::Up), ["k", "ArrowUp"];
    /// Move the cursor one cell right.
    cursor_right => "cursor-right", ViModeAction::Motion(ViModeMotion::Right), ["l", "ArrowRight"];
    /// Jump to column 0.
    line_start => "line-start", ViModeAction::Motion(ViModeMotion::LineStart), ["0"];
    /// Jump to the last column.
    line_end => "line-end", ViModeAction::Motion(ViModeMotion::LineEnd), ["$"];
    /// Jump to the first non-blank column.
    line_first_char => "line-first-char", ViModeAction::Motion(ViModeMotion::LineFirstChar), ["^"];
    /// Jump to the next word start.
    next_word => "next-word", ViModeAction::Motion(ViModeMotion::NextWord), ["w"];
    /// Jump to the previous word start.
    previous_word => "previous-word", ViModeAction::Motion(ViModeMotion::PreviousWord), ["b"];
    /// Jump to the next word end.
    next_word_end => "next-word-end", ViModeAction::Motion(ViModeMotion::NextWordEnd), ["e"];
    /// Jump to the next space-delimited word start.
    next_space => "next-space", ViModeAction::Motion(ViModeMotion::NextSpace), ["W"];
    /// Jump to the previous space-delimited word start.
    previous_space => "previous-space", ViModeAction::Motion(ViModeMotion::PreviousSpace), ["B"];
    /// Jump to the next space-delimited word end.
    next_space_end => "next-space-end", ViModeAction::Motion(ViModeMotion::NextSpaceEnd), ["E"];
    /// Jump to the top visible line.
    screen_top => "screen-top", ViModeAction::Motion(ViModeMotion::ScreenTop), ["H"];
    /// Jump to the middle visible line.
    screen_middle => "screen-middle", ViModeAction::Motion(ViModeMotion::ScreenMiddle), ["M"];
    /// Jump to the bottom visible line.
    screen_bottom => "screen-bottom", ViModeAction::Motion(ViModeMotion::ScreenBottom), ["L"];
    /// Jump to the previous paragraph boundary.
    previous_paragraph => "previous-paragraph", ViModeAction::Motion(ViModeMotion::PreviousParagraph), ["{"];
    /// Jump to the next paragraph boundary.
    next_paragraph => "next-paragraph", ViModeAction::Motion(ViModeMotion::NextParagraph), ["}"];
    /// Jump to the matching bracket.
    matching_bracket => "matching-bracket", ViModeAction::Motion(ViModeMotion::MatchingBracket), ["%"];
    /// Scroll to the oldest history line.
    history_top => "history-top", ViModeAction::Scroll(ViModeScroll::HistoryTop), ["g"];
    /// Scroll to the live tail.
    history_bottom => "history-bottom", ViModeAction::Scroll(ViModeScroll::HistoryBottom), ["G"];
    /// Scroll one page up.
    page_up => "page-up", ViModeAction::Scroll(ViModeScroll::PageUp), ["Ctrl+B"];
    /// Scroll one page down.
    page_down => "page-down", ViModeAction::Scroll(ViModeScroll::PageDown), ["Ctrl+F"];
    /// Scroll half a page up.
    half_page_up => "half-page-up", ViModeAction::Scroll(ViModeScroll::HalfPageUp), ["Ctrl+U"];
    /// Scroll half a page down.
    half_page_down => "half-page-down", ViModeAction::Scroll(ViModeScroll::HalfPageDown), ["Ctrl+D"];
    /// Scroll one line up.
    scroll_up => "scroll-up", ViModeAction::Scroll(ViModeScroll::ScrollUp), ["Ctrl+Y"];
    /// Scroll one line down.
    scroll_down => "scroll-down", ViModeAction::Scroll(ViModeScroll::ScrollDown), ["Ctrl+E"];
    /// Toggle a character-wise selection.
    toggle_selection => "toggle-selection", ViModeAction::Selection(ViModeSelection::Simple), ["v", "Space"];
    /// Toggle a line-wise selection.
    toggle_line_selection => "toggle-line-selection", ViModeAction::Selection(ViModeSelection::Lines), ["V"];
    /// Toggle a rectangular selection.
    toggle_rect_selection => "toggle-rect-selection", ViModeAction::Selection(ViModeSelection::Rect), ["Ctrl+V"];
    /// Copy the selection to the clipboard and leave vi mode.
    yank => "yank", ViModeAction::Yank, ["y", "Enter"];
    /// Leave vi mode.
    exit => "exit", ViModeAction::Exit, ["q", "Escape", "Ctrl+C"];
    /// Open the search-down prompt (tmux mode only for now).
    search_forward => "search-forward", ViModeAction::Prompt(ViModePromptDir::SearchForward), ["/"];
    /// Open the search-up prompt (tmux mode only for now).
    search_backward => "search-backward", ViModeAction::Prompt(ViModePromptDir::SearchBackward), ["?"];
    /// Repeat the previous search (tmux mode only for now).
    search_next => "search-next", ViModeAction::SearchStep(ViModeSearchStep::Next), ["n"];
    /// Repeat the previous search, reversed (tmux mode only for now).
    search_previous => "search-previous", ViModeAction::SearchStep(ViModeSearchStep::Previous), ["N"];
    /// Open the jump-to-char-forward prompt (tmux mode only for now).
    jump_forward => "jump-forward", ViModeAction::Prompt(ViModePromptDir::JumpForward), ["f"];
    /// Open the jump-to-char-backward prompt (tmux mode only for now).
    jump_backward => "jump-backward", ViModeAction::Prompt(ViModePromptDir::JumpBackward), ["F"];
    /// Open the jump-till-char-forward prompt (tmux mode only for now).
    jump_to_forward => "jump-to-forward", ViModeAction::Prompt(ViModePromptDir::JumpToForward), ["t"];
    /// Open the jump-till-char-backward prompt (tmux mode only for now).
    jump_to_backward => "jump-to-backward", ViModeAction::Prompt(ViModePromptDir::JumpToBackward), ["T"];
}

impl ViModeConfig {
    /// Detects keys bound to more than one action. Deterministic order via
    /// `BTreeMap`.
    pub(crate) fn validate_no_duplicate_keys(&self) -> Result<(), Vec<DuplicateViModeKey>> {
        let mut by_key: BTreeMap<ViModeKey, Vec<&'static str>> = BTreeMap::new();
        for (label, keys, _action) in self.bindings_iter() {
            for key in keys {
                by_key.entry(key.clone()).or_default().push(label);
            }
        }
        let dupes: Vec<DuplicateViModeKey> = by_key
            .into_iter()
            .filter(|(_, labels)| labels.len() >= 2)
            .map(|(key, actions)| DuplicateViModeKey { key, actions })
            .collect();
        if dupes.is_empty() { Ok(()) } else { Err(dupes) }
    }
}

/// TOML value shape for one action: a single key string or an array.
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

fn deser_keys<'de, D>(d: D) -> Result<Vec<ViModeKey>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) if s.is_empty() => Vec::new(),
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    };
    raw.iter()
        .map(|s| parse_vi_mode_key(s).map_err(DeError::custom))
        .collect()
}

fn ser_keys<S>(keys: &[ViModeKey], ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let strings: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    strings.serialize(ser)
}

fn parse_default_key(s: &str) -> ViModeKey {
    parse_vi_mode_key(s).unwrap_or_else(|e| panic!("invalid default vi-mode key {s:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(c: &str) -> ViModeKey {
        ViModeKey {
            ctrl: false,
            key: ViModeBaseKey::Char(c.to_string()),
        }
    }

    fn ctrl(c: &str) -> ViModeKey {
        ViModeKey {
            ctrl: true,
            key: ViModeBaseKey::Char(c.to_string()),
        }
    }

    #[test]
    fn bindings_iter_count_is_pinned_to_field_count() {
        // NOTE: drift guard — a ViModeConfig field without its
        // bindings_iter() entry silently unbinds the action.
        assert_eq!(ViModeConfig::default().bindings_iter().count(), 40);
    }

    #[test]
    fn parse_accepts_case_sensitive_chars_symbols_and_named() {
        assert_eq!(parse_vi_mode_key("v").unwrap(), plain("v"));
        assert_eq!(parse_vi_mode_key("V").unwrap(), plain("V"));
        assert_eq!(parse_vi_mode_key("$").unwrap(), plain("$"));
        assert_eq!(parse_vi_mode_key("+").unwrap(), plain("+"));
        assert_eq!(
            parse_vi_mode_key("Escape").unwrap(),
            ViModeKey {
                ctrl: false,
                key: ViModeBaseKey::Named(ViModeNamedKey::Escape),
            }
        );
    }

    #[test]
    fn parse_normalizes_literal_space_char_to_named_space() {
        assert_eq!(
            parse_vi_mode_key(" ").unwrap(),
            ViModeKey {
                ctrl: false,
                key: ViModeBaseKey::Named(ViModeNamedKey::Space),
            }
        );
    }

    #[test]
    fn parse_ctrl_lowercases_and_limits_domain() {
        assert_eq!(parse_vi_mode_key("Ctrl+F").unwrap(), ctrl("f"));
        assert_eq!(parse_vi_mode_key("ctrl+v").unwrap(), ctrl("v"));
        assert!(matches!(
            parse_vi_mode_key("Ctrl+$"),
            Err(ViModeKeyParseError::CtrlNonAlphanumeric(_))
        ));
    }

    #[test]
    fn parse_rejects_forbidden_modifiers_and_garbage() {
        assert!(matches!(
            parse_vi_mode_key("Cmd+x"),
            Err(ViModeKeyParseError::ForbiddenModifier(_))
        ));
        assert!(matches!(
            parse_vi_mode_key("Shift+v"),
            Err(ViModeKeyParseError::ForbiddenModifier(_))
        ));
        assert!(matches!(
            parse_vi_mode_key("Alt+j"),
            Err(ViModeKeyParseError::ForbiddenModifier(_))
        ));
        assert!(matches!(
            parse_vi_mode_key("ab"),
            Err(ViModeKeyParseError::UnknownKey(_))
        ));
        assert!(matches!(
            parse_vi_mode_key(""),
            Err(ViModeKeyParseError::Empty)
        ));
        assert!(matches!(
            parse_vi_mode_key("Ctrl+Shift+F"),
            Err(ViModeKeyParseError::TooManyTokens(_))
        ));
    }

    #[test]
    fn toml_accepts_string_array_and_unbind() {
        let cfg: ViModeConfig =
            toml::from_str("yank = \"Y\"\nexit = [\"q\", \"Ctrl+C\"]\nsearch-forward = \"\"\n")
                .unwrap();
        assert_eq!(cfg.yank, vec![plain("Y")]);
        assert_eq!(cfg.exit, vec![plain("q"), ctrl("c")]);
        assert!(cfg.search_forward.is_empty());
        assert_eq!(cfg.cursor_left, ViModeConfig::default().cursor_left);
    }

    #[test]
    fn toml_rejects_unknown_action() {
        assert!(toml::from_str::<ViModeConfig>("goto-line = \":\"\n").is_err());
    }

    #[test]
    fn default_has_no_duplicate_keys() {
        assert!(ViModeConfig::default().validate_no_duplicate_keys().is_ok());
    }

    #[test]
    fn duplicate_key_across_actions_is_detected() {
        let cfg: ViModeConfig = toml::from_str("yank = \"x\"\nexit = [\"x\", \"q\"]\n").unwrap();
        let dupes = cfg.validate_no_duplicate_keys().unwrap_err();
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0].key, plain("x"));
        assert!(dupes[0].actions.contains(&"yank"));
        assert!(dupes[0].actions.contains(&"exit"));
    }

    #[test]
    fn defaults_match_spec_spot_checks() {
        let cfg = ViModeConfig::default();
        assert_eq!(cfg.cursor_left[0], plain("h"));
        assert_eq!(
            cfg.cursor_left[1],
            ViModeKey {
                ctrl: false,
                key: ViModeBaseKey::Named(ViModeNamedKey::ArrowLeft),
            }
        );
        assert_eq!(cfg.line_end, vec![plain("$")]);
        assert_eq!(cfg.next_space, vec![plain("W")]);
        assert_eq!(cfg.page_down, vec![ctrl("f")]);
        assert_eq!(cfg.toggle_rect_selection, vec![ctrl("v")]);
        assert_eq!(
            cfg.yank,
            vec![
                plain("y"),
                ViModeKey {
                    ctrl: false,
                    key: ViModeBaseKey::Named(ViModeNamedKey::Enter)
                }
            ]
        );
        assert_eq!(cfg.search_next, vec![plain("n")]);
        assert_eq!(cfg.search_previous, vec![plain("N")]);
    }

    #[test]
    fn display_roundtrips_tokens() {
        assert_eq!(plain("$").to_string(), "$");
        assert_eq!(ctrl("f").to_string(), "Ctrl+f");
        assert_eq!(
            ViModeKey {
                ctrl: false,
                key: ViModeBaseKey::Named(ViModeNamedKey::ArrowUp),
            }
            .to_string(),
            "ArrowUp"
        );
    }
}
