//! A keyboard chord declared as passthrough for a webview (crossterm-typed).

use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde::Serialize;
use serde::ser::SerializeMap;

/// A modifier chord a webview lets through to the app while focused.
///
/// Serializes to the host wire shape `{ "mods": [...], "key": "..." }` consumed
/// by the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChord {
    /// The required modifiers.
    pub mods: KeyModifiers,
    /// The key code.
    pub code: KeyCode,
}

impl Serialize for KeyChord {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut mods = Vec::new();
        if self.mods.contains(KeyModifiers::ALT) {
            mods.push("alt");
        }
        if self.mods.contains(KeyModifiers::CONTROL) {
            mods.push("ctrl");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            mods.push("shift");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            mods.push("meta");
        }
        let key = match self.code {
            KeyCode::Char(c) => c.to_ascii_lowercase().to_string(),
            KeyCode::Tab => "tab".to_owned(),
            KeyCode::BackTab => "backtab".to_owned(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_lowercase(),
        };
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("mods", &mods)?;
        m.serialize_entry("key", &key)?;
        m.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_alt_char_to_wire_shape() {
        let c = KeyChord { mods: KeyModifiers::ALT, code: KeyCode::Char('H') };
        assert_eq!(serde_json::to_value(c).unwrap(), json!({"mods": ["alt"], "key": "h"}));
    }

    #[test]
    fn serializes_ctrl_tab_and_fkey() {
        let tab = KeyChord { mods: KeyModifiers::CONTROL, code: KeyCode::Tab };
        assert_eq!(serde_json::to_value(tab).unwrap(), json!({"mods": ["ctrl"], "key": "tab"}));
        let f5 = KeyChord { mods: KeyModifiers::NONE, code: KeyCode::F(5) };
        assert_eq!(serde_json::to_value(f5).unwrap(), json!({"mods": [], "key": "f5"}));
    }
}
