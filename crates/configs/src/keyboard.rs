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
pub struct KeyboardConfig {
    /// Which Option key(s) act as Meta on macOS.
    pub option_as_alt: OptionAsAlt,
}

/// Per-field-optional patch produced by parsing `[keyboard]` from
/// `config.toml`. Missing keys fall through to the defaults.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyboardPatch {
    pub(crate) option_as_alt: Option<OptionAsAlt>,
}

impl KeyboardPatch {
    pub(crate) fn apply_to(self, mut base: KeyboardConfig) -> KeyboardConfig {
        if let Some(v) = self.option_as_alt {
            base.option_as_alt = v;
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none() {
        assert_eq!(KeyboardConfig::default().option_as_alt, OptionAsAlt::None);
    }

    #[test]
    fn parses_each_lowercase_value() {
        for (s, expected) in [
            ("none", OptionAsAlt::None),
            ("left", OptionAsAlt::Left),
            ("right", OptionAsAlt::Right),
            ("both", OptionAsAlt::Both),
        ] {
            let patch: KeyboardPatch =
                toml::from_str(&format!(r#"option_as_alt = "{s}""#)).unwrap();
            assert_eq!(patch.option_as_alt, Some(expected));
        }
    }

    #[test]
    fn patch_overrides_present_field() {
        let patch = KeyboardPatch {
            option_as_alt: Some(OptionAsAlt::Both),
        };
        let merged = patch.apply_to(KeyboardConfig::default());
        assert_eq!(merged.option_as_alt, OptionAsAlt::Both);
    }

    #[test]
    fn empty_patch_keeps_default() {
        let patch = KeyboardPatch::default();
        let merged = patch.apply_to(KeyboardConfig::default());
        assert_eq!(merged, KeyboardConfig::default());
    }

    #[test]
    fn rejects_unknown_value() {
        let err = toml::from_str::<KeyboardPatch>(r#"option_as_alt = "meta""#).err();
        assert!(err.is_some(), "unknown enum value must be a parse error");
    }

    #[test]
    fn rejects_unknown_field() {
        let err = toml::from_str::<KeyboardPatch>(r#"option_as_alt2 = "both""#).err();
        assert!(
            err.is_some(),
            "a misspelled key in [keyboard] must be a parse error, not silently ignored"
        );
    }
}
