//! Keyboard input configuration: macOS Option-as-Meta behavior.

use serde::{Deserialize, Serialize};

/// Which Option/Alt key(s) macOS treats as Meta (Alt) instead of composing
/// into special characters. Mirrors winit's `OptionAsAlt`; has no effect on
/// non-macOS platforms, where Alt is always Meta.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OptionAsAlt {
    /// Neither Option key acts as Meta; both compose normally (default).
    #[default]
    None,
    /// Only the left Option key acts as Meta; the right one composes.
    Left,
    /// Only the right Option key acts as Meta; the left one composes.
    Right,
    /// Both Option keys act as Meta.
    Both,
}

/// Fully-resolved `[keyboard]` config block.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardConfig {
    /// Which Option key(s) act as Meta on macOS.
    pub option_as_alt: OptionAsAlt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none() {
        assert_eq!(KeyboardConfig::default().option_as_alt, OptionAsAlt::None);
    }

    #[test]
    fn parses_value() {
        let cfg: KeyboardConfig = toml::from_str(r#"option_as_alt = "both""#).unwrap();
        assert_eq!(cfg.option_as_alt, OptionAsAlt::Both);
    }

    #[test]
    fn empty_is_default() {
        let cfg: KeyboardConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, KeyboardConfig::default());
    }

    #[test]
    fn rejects_unknown_value() {
        assert!(toml::from_str::<KeyboardConfig>(r#"option_as_alt = "meta""#).is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(
            toml::from_str::<KeyboardConfig>(r#"option_as_alt2 = "both""#).is_err(),
            "a misspelled key must error under deny_unknown_fields"
        );
    }
}
