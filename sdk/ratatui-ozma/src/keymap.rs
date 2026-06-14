//! The navigation keymap shared by the app's native key matching and the page
//! glue (`__ozma.keys`).
//!
//! A [`NavKeymap`] is the single source of truth for "which chord moves focus
//! which way": the app uses [`NavKeymap::match_key`] on its own key events while
//! a native widget is focused, and pushes the same map to the page glue via
//! [`crate::WebviewHandle::set_nav_keys`] so a focused webview escapes on the
//! same chord. Its serialized form matches the glue's `{ mods, keys }` shape.

use crate::focus::Direction;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Serialize;
use serde::ser::SerializeMap;

/// A keyboard modifier required by a [`NavKeymap`] chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    /// The Alt / Option key.
    Alt,
    /// The Control key.
    Ctrl,
    /// The Shift key.
    Shift,
    /// The Meta / Command / Super key.
    Meta,
}

impl Modifier {
    fn js_name(self) -> &'static str {
        match self {
            Modifier::Alt => "alt",
            Modifier::Ctrl => "ctrl",
            Modifier::Shift => "shift",
            Modifier::Meta => "meta",
        }
    }

    fn key_modifier(self) -> KeyModifiers {
        match self {
            Modifier::Alt => KeyModifiers::ALT,
            Modifier::Ctrl => KeyModifiers::CONTROL,
            Modifier::Shift => KeyModifiers::SHIFT,
            Modifier::Meta => KeyModifiers::SUPER,
        }
    }
}

/// A key a [`NavKeymap`] binds to a [`Direction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    /// A character key (matched ASCII-case-insensitively).
    Char(char),
    /// The Left arrow.
    ArrowLeft,
    /// The Down arrow.
    ArrowDown,
    /// The Up arrow.
    ArrowUp,
    /// The Right arrow.
    ArrowRight,
}

impl NavKey {
    /// The `KeyboardEvent.key` name (lowercased) the page glue matches against.
    fn js_name(self) -> String {
        match self {
            NavKey::Char(c) => c.to_ascii_lowercase().to_string(),
            NavKey::ArrowLeft => "arrowleft".to_owned(),
            NavKey::ArrowDown => "arrowdown".to_owned(),
            NavKey::ArrowUp => "arrowup".to_owned(),
            NavKey::ArrowRight => "arrowright".to_owned(),
        }
    }

    fn matches(self, code: KeyCode) -> bool {
        match (self, code) {
            (NavKey::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(&b),
            (NavKey::ArrowLeft, KeyCode::Left) => true,
            (NavKey::ArrowDown, KeyCode::Down) => true,
            (NavKey::ArrowUp, KeyCode::Up) => true,
            (NavKey::ArrowRight, KeyCode::Right) => true,
            _ => false,
        }
    }
}

/// A navigation keymap: a shared modifier set plus key→direction bindings.
///
/// Drives both native key matching ([`match_key`](Self::match_key)) and the page
/// glue (pushed via [`crate::WebviewHandle::set_nav_keys`]). Serializes to the
/// glue's `{ "mods": [...], "keys": { name: dir } }` form, where every chord
/// requires all of `mods`.
#[derive(Debug, Clone, PartialEq)]
pub struct NavKeymap {
    mods: Vec<Modifier>,
    bindings: Vec<(NavKey, Direction)>,
}

impl NavKeymap {
    /// Creates an empty keymap whose chords all require `mods`.
    pub fn new(mods: impl IntoIterator<Item = Modifier>) -> Self {
        Self {
            mods: mods.into_iter().collect(),
            bindings: Vec::new(),
        }
    }

    /// Binds `key` to `direction`.
    pub fn bind(mut self, key: NavKey, direction: Direction) -> Self {
        self.bindings.push((key, direction));
        self
    }

    /// The `Alt+h/j/k/l` chord (the SDK's conventional default).
    pub fn alt_hjkl() -> Self {
        Self::hjkl([Modifier::Alt])
    }

    /// The `Ctrl+h/j/k/l` chord.
    pub fn ctrl_hjkl() -> Self {
        Self::hjkl([Modifier::Ctrl])
    }

    /// Bare arrow keys (the most reliable scheme across terminals).
    pub fn arrows() -> Self {
        Self::new([])
            .bind(NavKey::ArrowLeft, Direction::Left)
            .bind(NavKey::ArrowDown, Direction::Down)
            .bind(NavKey::ArrowUp, Direction::Up)
            .bind(NavKey::ArrowRight, Direction::Right)
    }

    /// Resolves a key event to a bound [`Direction`], or `None`.
    ///
    /// Matches when the event carries every required modifier and its key code
    /// matches a binding — mirroring the page glue's match semantics.
    pub fn match_key(&self, key: &KeyEvent) -> Option<Direction> {
        if !self
            .mods
            .iter()
            .all(|m| key.modifiers.contains(m.key_modifier()))
        {
            return None;
        }
        self.bindings
            .iter()
            .find(|(k, _)| k.matches(key.code))
            .map(|(_, d)| *d)
    }

    fn hjkl(mods: impl IntoIterator<Item = Modifier>) -> Self {
        Self::new(mods)
            .bind(NavKey::Char('h'), Direction::Left)
            .bind(NavKey::Char('j'), Direction::Down)
            .bind(NavKey::Char('k'), Direction::Up)
            .bind(NavKey::Char('l'), Direction::Right)
    }
}

impl Serialize for NavKeymap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mods: Vec<&str> = self.mods.iter().map(|m| m.js_name()).collect();
        let keys: std::collections::BTreeMap<String, Direction> = self
            .bindings
            .iter()
            .map(|(k, d)| (k.js_name(), *d))
            .collect();
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("mods", &mods)?;
        map.serialize_entry("keys", &keys)?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn arrows_serializes_to_glue_shape() {
        assert_eq!(
            serde_json::to_value(NavKeymap::arrows()).unwrap(),
            json!({
                "mods": [],
                "keys": {"arrowleft": "left", "arrowdown": "down", "arrowup": "up", "arrowright": "right"}
            })
        );
    }

    #[test]
    fn alt_hjkl_serializes_to_glue_shape() {
        assert_eq!(
            serde_json::to_value(NavKeymap::alt_hjkl()).unwrap(),
            json!({
                "mods": ["alt"],
                "keys": {"h": "left", "j": "down", "k": "up", "l": "right"}
            })
        );
    }

    #[test]
    fn ctrl_hjkl_uses_the_ctrl_modifier() {
        let v = serde_json::to_value(NavKeymap::ctrl_hjkl()).unwrap();
        assert_eq!(v["mods"], json!(["ctrl"]));
    }

    #[test]
    fn match_key_alt_hjkl_requires_alt() {
        let km = NavKeymap::alt_hjkl();
        assert_eq!(
            km.match_key(&key(KeyCode::Char('h'), KeyModifiers::ALT)),
            Some(Direction::Left)
        );
        assert_eq!(
            km.match_key(&key(KeyCode::Char('l'), KeyModifiers::ALT)),
            Some(Direction::Right)
        );
        assert_eq!(
            km.match_key(&key(KeyCode::Char('h'), KeyModifiers::NONE)),
            None,
            "bare h must not match an Alt chord"
        );
        assert_eq!(
            km.match_key(&key(KeyCode::Char('x'), KeyModifiers::ALT)),
            None,
            "an unbound key returns None"
        );
    }

    #[test]
    fn match_key_arrows() {
        let km = NavKeymap::arrows();
        assert_eq!(
            km.match_key(&key(KeyCode::Left, KeyModifiers::NONE)),
            Some(Direction::Left)
        );
        assert_eq!(
            km.match_key(&key(KeyCode::Up, KeyModifiers::NONE)),
            Some(Direction::Up)
        );
        assert_eq!(
            km.match_key(&key(KeyCode::Char('h'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn custom_builder_binds_a_chord() {
        let km = NavKeymap::new([Modifier::Ctrl]).bind(NavKey::Char('a'), Direction::Up);
        assert_eq!(
            km.match_key(&key(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            Some(Direction::Up)
        );
    }
}
