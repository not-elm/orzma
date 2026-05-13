//! Shortcut domain types: keys, modifiers, chords, prefix, bindings, actions.

use serde::{Deserialize, Serialize};

/// Logical key. v0 covers ASCII characters and a small set of named keys.
#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
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
    /// Forward-compatibility variant for unknown logical key names.
    Other(String),
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
