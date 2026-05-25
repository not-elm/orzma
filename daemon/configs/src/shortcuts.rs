//! Shortcut domain types: keys, modifiers, chords, prefix, bindings, actions.

use serde::{Deserialize, Serialize};

/// Logical key. v0 covers ASCII characters and a small set of named keys.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
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
#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Clone, Debug, Default)]
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
#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Clone, Debug)]
pub struct KeyChord {
    /// Logical key.
    pub key: Key,
    /// Held modifiers.
    #[serde(default)]
    pub modifiers: Modifiers,
}

/// User-facing shortcut configuration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Shortcuts {
    /// The "armed" prefix chord.
    pub prefix: Prefix,
    /// Bindings dispatched while armed.
    pub bindings: Vec<Binding>,
    /// Milliseconds during which a `repeatable` binding accepts its trigger
    /// key without the prefix.
    #[serde(default = "default_repeat_timeout_ms")]
    pub repeat_timeout_ms: u64,
}

fn default_repeat_timeout_ms() -> u64 {
    500
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            prefix: Prefix {
                chord: KeyChord {
                    key: Key::Char('b'),
                    modifiers: Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                },
                timeout_ms: 2000,
            },
            bindings: vec![
                Binding {
                    chord: KeyChord {
                        key: Key::Char('x'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::ClosePane,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('s'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::SplitPane {
                        direction: SplitDirection::Horizontal,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('v'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::SplitPane {
                        direction: SplitDirection::Vertical,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('c'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::NewTerminalActivity,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('w'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::ChooseTree,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('&'),
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::CloseActivity,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('s'),
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::BreakActivityToPane {
                        direction: SplitDirection::Horizontal,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('v'),
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::BreakActivityToPane {
                        direction: SplitDirection::Vertical,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char(']'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusActivity {
                        offset: ActivityOffset::Next,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('['),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::EnterCopyMode,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char(';'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusActivity {
                        offset: ActivityOffset::Prev,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('n'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindow {
                        offset: WindowOffset::Next,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('p'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindow {
                        offset: WindowOffset::Prev,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('0'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 0 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('1'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 1 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('2'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 2 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('3'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 3 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('4'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 4 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('5'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 5 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('6'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 6 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('7'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 7 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('8'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 8 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('9'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusWindowNumber { index: 9 },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('h'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusPane {
                        direction: Direction::Left,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('j'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusPane {
                        direction: Direction::Down,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('k'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusPane {
                        direction: Direction::Up,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('l'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::FocusPane {
                        direction: Direction::Right,
                    },
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::ArrowLeft,
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::ResizePane {
                        direction: Direction::Left,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::ArrowRight,
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::ResizePane {
                        direction: Direction::Right,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::ArrowUp,
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::ResizePane {
                        direction: Direction::Up,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::ArrowDown,
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::ResizePane {
                        direction: Direction::Down,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char(','),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::RenameWindow,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('c'),
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::NewWindow,
                    repeatable: false,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('{'),
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::SwapPane {
                        offset: SwapOffset::Prev,
                    },
                    repeatable: true,
                },
                Binding {
                    chord: KeyChord {
                        key: Key::Char('}'),
                        modifiers: Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    },
                    action: Action::SwapPane {
                        offset: SwapOffset::Next,
                    },
                    repeatable: true,
                },
            ],
            repeat_timeout_ms: 500,
        }
    }
}

/// Prefix chord and timeout used to "arm" the shortcut dispatcher.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Prefix {
    /// Chord that arms the dispatcher.
    #[serde(flatten)]
    pub chord: KeyChord,
    /// Milliseconds to wait for the follow-up key before disarming.
    pub timeout_ms: u64,
}

/// One armed-mode binding: a chord to listen for and the action to dispatch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// Chord pressed while armed.
    #[serde(flatten)]
    pub chord: KeyChord,
    /// Action dispatched when the chord matches.
    pub action: Action,
    /// Whether the trigger key can be repeated without the prefix within
    /// `Shortcuts::repeat_timeout_ms` of the previous trigger.
    #[serde(default)]
    pub repeatable: bool,
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
    fn prefix_deserializes_with_default_modifiers() {
        let toml = r#"
            key = "b"
            modifiers = { ctrl = true }
            timeout_ms = 2000
        "#;
        let p: Prefix = toml::from_str(toml).unwrap();
        assert_eq!(p.chord.key, Key::Char('b'));
        assert!(p.chord.modifiers.ctrl);
        assert!(!p.chord.modifiers.shift);
        assert_eq!(p.timeout_ms, 2000);
    }

    #[test]
    fn binding_deserializes_close_pane() {
        let toml = r#"
            key = "x"
            action = { type = "close-pane" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert_eq!(b.chord.key, Key::Char('x'));
        assert_eq!(b.chord.modifiers, Modifiers::default());
        assert!(matches!(b.action, Action::ClosePane));
    }

    #[test]
    fn binding_deserializes_split_pane_with_direction() {
        let toml = r#"
            key = "%"
            action = { type = "split-pane", direction = "horizontal" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert!(matches!(
            b.action,
            Action::SplitPane {
                direction: SplitDirection::Horizontal
            }
        ));
    }

    #[test]
    fn binding_deserializes_focus_window_number() {
        let toml = r#"
            key = "0"
            action = { type = "focus-window-number", index = 0 }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert!(matches!(b.action, Action::FocusWindowNumber { index: 0 }));
    }

    #[test]
    fn binding_deserializes_new_terminal_activity() {
        let toml = r#"
            key = "c"
            action = { type = "new-terminal-activity" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert_eq!(b.chord.key, Key::Char('c'));
        assert_eq!(b.chord.modifiers, Modifiers::default());
        assert!(matches!(b.action, Action::NewTerminalActivity));
    }

    #[test]
    fn binding_deserializes_close_activity() {
        let toml = r#"
            key = "w"
            action = { type = "close-activity" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert_eq!(b.chord.key, Key::Char('w'));
        assert_eq!(b.chord.modifiers, Modifiers::default());
        assert!(matches!(b.action, Action::CloseActivity));
    }

    #[test]
    fn binding_deserializes_focus_activity_next() {
        let toml = r#"
            key = "]"
            action = { type = "focus-activity", offset = "next" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert_eq!(b.chord.key, Key::Char(']'));
        assert_eq!(b.chord.modifiers, Modifiers::default());
        assert!(matches!(
            b.action,
            Action::FocusActivity {
                offset: ActivityOffset::Next
            }
        ));
    }

    #[test]
    fn binding_deserializes_focus_activity_prev() {
        let toml = r#"
            key = "["
            action = { type = "focus-activity", offset = "prev" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert_eq!(b.chord.key, Key::Char('['));
        assert_eq!(b.chord.modifiers, Modifiers::default());
        assert!(matches!(
            b.action,
            Action::FocusActivity {
                offset: ActivityOffset::Prev
            }
        ));
    }

    #[test]
    fn binding_deserializes_repeatable_flag() {
        let toml = r#"
            key = "ArrowRight"
            modifiers = { ctrl = true }
            repeatable = true
            action = { type = "resize-pane", direction = "right" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert!(b.repeatable);
        assert!(matches!(
            b.action,
            Action::ResizePane {
                direction: Direction::Right
            }
        ));
    }

    #[test]
    fn binding_repeatable_defaults_false_when_omitted() {
        let toml = r#"
            key = "x"
            action = { type = "close-pane" }
        "#;
        let b: Binding = toml::from_str(toml).unwrap();
        assert!(!b.repeatable);
    }

    #[test]
    fn shortcuts_repeat_timeout_ms_defaults_to_500_when_omitted() {
        let toml = r#"
            bindings = []

            [prefix]
            key = "b"
            modifiers = { ctrl = true }
            timeout_ms = 2000
        "#;
        let s: Shortcuts = toml::from_str(toml).unwrap();
        assert_eq!(s.repeat_timeout_ms, 500);
    }

    #[test]
    fn default_bindings_include_new_window_on_shift_c() {
        let shortcuts = Shortcuts::default();
        let found = shortcuts.bindings.iter().any(|b| {
            b.chord.key == Key::Char('c')
                && b.chord.modifiers.shift
                && !b.chord.modifiers.ctrl
                && !b.chord.modifiers.alt
                && !b.chord.modifiers.meta
                && matches!(b.action, Action::NewWindow)
                && !b.repeatable
        });
        assert!(found, "default bindings must include Shift+C -> NewWindow");
    }

    #[test]
    fn default_bindings_include_w_choose_tree() {
        let s = Shortcuts::default();
        let found = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char('w')
                && !b.chord.modifiers.shift
                && !b.chord.modifiers.ctrl
                && !b.chord.modifiers.alt
                && !b.chord.modifiers.meta
                && matches!(b.action, Action::ChooseTree)
                && !b.repeatable
        });
        assert!(found, "default bindings must include w -> ChooseTree");
    }

    #[test]
    fn default_bindings_relocate_close_activity_to_ampersand() {
        let s = Shortcuts::default();
        let found = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char('&')
                && b.chord.modifiers.shift
                && !b.chord.modifiers.ctrl
                && !b.chord.modifiers.alt
                && !b.chord.modifiers.meta
                && matches!(b.action, Action::CloseActivity)
                && !b.repeatable
        });
        assert!(
            found,
            "default bindings must include Shift+& -> CloseActivity"
        );

        let leftover = s
            .bindings
            .iter()
            .any(|b| b.chord.key == Key::Char('w') && matches!(b.action, Action::CloseActivity));
        assert!(!leftover, "old w -> CloseActivity binding must be removed");
    }

    #[test]
    fn default_bindings_include_swap_pane_prev_and_next() {
        let s = Shortcuts::default();
        let prev = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char('{')
                && b.chord.modifiers.shift
                && matches!(
                    b.action,
                    Action::SwapPane {
                        offset: SwapOffset::Prev
                    }
                )
                && b.repeatable
        });
        assert!(prev, "missing Prefix+{{ -> SwapPane(Prev) (repeatable)");
        let next = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char('}')
                && b.chord.modifiers.shift
                && matches!(
                    b.action,
                    Action::SwapPane {
                        offset: SwapOffset::Next
                    }
                )
                && b.repeatable
        });
        assert!(next, "missing Prefix+}} -> SwapPane(Next) (repeatable)");
    }

    #[test]
    fn default_bindings_rebind_open_bracket_to_enter_copy_mode() {
        let s = Shortcuts::default();
        let bracket_to_enter = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char('[')
                && b.chord.modifiers == Modifiers::default()
                && matches!(b.action, Action::EnterCopyMode)
                && !b.repeatable
        });
        assert!(
            bracket_to_enter,
            "default bindings must include Prefix+[ -> EnterCopyMode"
        );
        let leftover_prev_on_bracket = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char('[')
                && matches!(
                    b.action,
                    Action::FocusActivity {
                        offset: ActivityOffset::Prev
                    }
                )
        });
        assert!(
            !leftover_prev_on_bracket,
            "the old [ -> FocusActivity::Prev binding must be removed"
        );
    }

    #[test]
    fn default_bindings_rebind_focus_activity_prev_to_semicolon() {
        let s = Shortcuts::default();
        let semicolon_to_prev = s.bindings.iter().any(|b| {
            b.chord.key == Key::Char(';')
                && b.chord.modifiers == Modifiers::default()
                && matches!(
                    b.action,
                    Action::FocusActivity {
                        offset: ActivityOffset::Prev
                    }
                )
                && !b.repeatable
        });
        assert!(
            semicolon_to_prev,
            "default bindings must include Prefix+; -> FocusActivity Prev"
        );
    }

    #[test]
    fn default_shortcuts_serializes_to_stable_json() {
        let json = serde_json::to_string(&Shortcuts::default()).unwrap();
        assert_eq!(
            json,
            r#"{"prefix":{"key":"b","modifiers":{"ctrl":true,"shift":false,"alt":false,"meta":false},"timeout_ms":2000},"bindings":[{"key":"x","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"close-pane"},"repeatable":false},{"key":"s","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"split-pane","direction":"horizontal"},"repeatable":false},{"key":"v","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"split-pane","direction":"vertical"},"repeatable":false},{"key":"c","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"new-terminal-activity"},"repeatable":false},{"key":"w","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"choose-tree"},"repeatable":false},{"key":"&","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"close-activity"},"repeatable":false},{"key":"s","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"break-activity-to-pane","direction":"horizontal"},"repeatable":false},{"key":"v","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"break-activity-to-pane","direction":"vertical"},"repeatable":false},{"key":"]","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-activity","offset":"next"},"repeatable":false},{"key":"[","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"enter-copy-mode"},"repeatable":false},{"key":";","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-activity","offset":"prev"},"repeatable":false},{"key":"n","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window","offset":"next"},"repeatable":true},{"key":"p","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window","offset":"prev"},"repeatable":true},{"key":"0","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":0},"repeatable":false},{"key":"1","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":1},"repeatable":false},{"key":"2","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":2},"repeatable":false},{"key":"3","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":3},"repeatable":false},{"key":"4","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":4},"repeatable":false},{"key":"5","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":5},"repeatable":false},{"key":"6","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":6},"repeatable":false},{"key":"7","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":7},"repeatable":false},{"key":"8","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":8},"repeatable":false},{"key":"9","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-window-number","index":9},"repeatable":false},{"key":"h","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-pane","direction":"left"},"repeatable":false},{"key":"j","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-pane","direction":"down"},"repeatable":false},{"key":"k","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-pane","direction":"up"},"repeatable":false},{"key":"l","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"focus-pane","direction":"right"},"repeatable":false},{"key":"ArrowLeft","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"resize-pane","direction":"left"},"repeatable":true},{"key":"ArrowRight","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"resize-pane","direction":"right"},"repeatable":true},{"key":"ArrowUp","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"resize-pane","direction":"up"},"repeatable":true},{"key":"ArrowDown","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"resize-pane","direction":"down"},"repeatable":true},{"key":",","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":false},"action":{"type":"rename-window"},"repeatable":false},{"key":"c","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"new-window"},"repeatable":false},{"key":"{","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"swap-pane","offset":"prev"},"repeatable":true},{"key":"}","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":false},"action":{"type":"swap-pane","offset":"next"},"repeatable":true}],"repeat_timeout_ms":500}"#
        );
    }
}
