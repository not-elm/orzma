//! TOML-deserialization layer for `OzmuxConfigs`. Holds optional sections
//! and merges them onto a baseline `OzmuxConfigs::default()`.

// These items are intentionally unused until `OzmuxConfigs::load` is wired
// up in a subsequent task. The `#[expect]` will start failing at that point,
// signalling that the attribute can be removed.
#![cfg_attr(not(test), expect(dead_code, reason = "consumed by OzmuxConfigs::load in a subsequent task"))]

use crate::OzmuxConfigs;
use crate::shortcuts::{Binding, Prefix};
use crate::theme::ThemePatch;
use serde::Deserialize;

/// Top-level TOML shape: every section is optional.
#[derive(Deserialize, Default)]
pub(crate) struct RawConfigs {
    pub(crate) shortcuts: Option<RawShortcuts>,
    pub(crate) theme: Option<ThemePatch>,
}

/// `[shortcuts]` shape with each subfield optional.
#[derive(Deserialize, Default)]
pub(crate) struct RawShortcuts {
    pub(crate) prefix: Option<Prefix>,
    pub(crate) bindings: Option<Vec<Binding>>,
}

impl RawConfigs {
    /// Applies any populated fields onto `base` and returns the merged result.
    /// Within the `shortcuts` section, `prefix` and `bindings` are independently
    /// optional; `bindings` is full-replace. The `theme` section uses
    /// `ThemePatch::apply_to` for per-field merge.
    pub(crate) fn apply_to(self, mut base: OzmuxConfigs) -> OzmuxConfigs {
        if let Some(raw) = self.shortcuts {
            if let Some(prefix) = raw.prefix {
                base.shortcuts.prefix = prefix;
            }
            if let Some(bindings) = raw.bindings {
                base.shortcuts.bindings = bindings;
            }
        }
        if let Some(patch) = self.theme {
            base.theme = patch.apply_to(base.theme);
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::Action;

    #[test]
    fn empty_raw_returns_defaults() {
        let raw = RawConfigs::default();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.shortcuts.bindings.len(), 1);
        assert!(matches!(merged.shortcuts.bindings[0].action, Action::ClosePane));
    }

    #[test]
    fn theme_patch_preserves_unset_fields() {
        let raw: RawConfigs = toml::from_str(r##"
            [theme]
            accent = "#deadbe"
        "##).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.theme.accent, "#deadbe");
        assert_eq!(merged.theme.background, "#1a1b26");
    }

    #[test]
    fn bindings_fully_replace_defaults() {
        let raw: RawConfigs = toml::from_str(r#"
            [[shortcuts.bindings]]
            key = "y"
            action = { type = "close-window" }
        "#).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.shortcuts.bindings.len(), 1);
        assert!(matches!(merged.shortcuts.bindings[0].action, Action::CloseWindow));
    }
}
