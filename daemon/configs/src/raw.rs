//! TOML-deserialization layer for `OzmuxConfigs`. Holds optional sections
//! and merges them onto a baseline `OzmuxConfigs::default()`.

use crate::OzmuxConfigs;
use crate::OzmuxConfigsError;
use crate::OzmuxConfigsResult;
use crate::font::FontPatch;
use crate::shortcuts::{Binding, Prefix};
use crate::theme::ThemePatch;
use serde::Deserialize;
use std::collections::HashSet;

/// Top-level TOML shape: every section is optional.
#[derive(Deserialize, Default)]
pub(crate) struct RawConfigs {
    pub(crate) shortcuts: Option<RawShortcuts>,
    pub(crate) theme: Option<ThemePatch>,
    pub(crate) font: Option<FontPatch>,
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
    /// optional; `bindings` is full-replace. The `theme` and `font` sections use
    /// their respective `Patch::apply_to` for per-field merge.
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
        if let Some(patch) = self.font {
            base.font = patch.apply_to(base.font);
        }
        base
    }
}

/// Walks `configs.shortcuts.bindings` and rejects duplicate chords.
pub(crate) fn validate(configs: &OzmuxConfigs) -> OzmuxConfigsResult {
    let mut seen: HashSet<&crate::shortcuts::KeyChord> = HashSet::new();
    for b in &configs.shortcuts.bindings {
        if !seen.insert(&b.chord) {
            return Err(OzmuxConfigsError::DuplicateBinding {
                chord: b.chord.clone(),
            });
        }
    }
    let size = configs.font.size;
    if !(size > 0.0 && size <= 200.0) {
        return Err(OzmuxConfigsError::InvalidFontSize { size });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::Action;

    #[test]
    fn empty_raw_returns_defaults() {
        let raw = RawConfigs::default();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.shortcuts.bindings.len(), 30);
        assert!(matches!(
            merged.shortcuts.bindings[0].action,
            Action::ClosePane
        ));
    }

    #[test]
    fn theme_patch_preserves_unset_fields() {
        let raw: RawConfigs = toml::from_str(
            r##"
            [theme]
            accent = "#deadbe"
        "##,
        )
        .unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.theme.accent, "#deadbe");
        assert_eq!(merged.theme.background, "#1a1b26");
    }

    #[test]
    fn bindings_fully_replace_defaults() {
        let raw: RawConfigs = toml::from_str(
            r#"
            [[shortcuts.bindings]]
            key = "y"
            action = { type = "close-window" }
        "#,
        )
        .unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.shortcuts.bindings.len(), 1);
        assert!(matches!(
            merged.shortcuts.bindings[0].action,
            Action::CloseWindow
        ));
    }

    #[test]
    fn validate_rejects_duplicate_binding() {
        let raw: RawConfigs = toml::from_str(
            r#"
            [[shortcuts.bindings]]
            key = "x"
            action = { type = "close-pane" }

            [[shortcuts.bindings]]
            key = "x"
            action = { type = "close-window" }
        "#,
        )
        .unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        let err = validate(&merged).unwrap_err();
        assert!(matches!(
            err,
            crate::OzmuxConfigsError::DuplicateBinding { .. }
        ));
    }

    #[test]
    fn validate_accepts_modifier_in_binding() {
        let raw: RawConfigs = toml::from_str(
            r#"
            [[shortcuts.bindings]]
            key = "x"
            modifiers = { shift = true }
            action = { type = "close-pane" }
        "#,
        )
        .unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(
            merged.shortcuts.bindings[0].chord.modifiers.shift,
            "shift modifier must survive parsing and merging"
        );
        validate(&merged).unwrap();
    }

    #[test]
    fn validate_accepts_default_config() {
        validate(&OzmuxConfigs::default()).unwrap();
    }

    #[test]
    fn font_section_merges_and_falls_back() {
        let raw: RawConfigs = toml::from_str(
            r#"
            [font]
            size = 15.0
            [font.normal]
            family = "Hack Nerd Font"
            style = "Regular"
            [font.offset]
            x = 0
        "#,
        )
        .unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.font.size, 15.0);
        assert_eq!(merged.font.normal_family, "Hack Nerd Font");
        assert_eq!(merged.font.bold_family, "Hack Nerd Font");
    }

    #[test]
    fn absent_font_section_keeps_defaults() {
        let raw = RawConfigs::default();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.font, crate::font::FontConfig::default());
    }

    #[test]
    fn validate_rejects_zero_font_size() {
        let raw: RawConfigs = toml::from_str("[font]\nsize = 0.0\n").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        let err = validate(&merged).unwrap_err();
        assert!(matches!(
            err,
            crate::OzmuxConfigsError::InvalidFontSize { .. }
        ));
    }

    #[test]
    fn validate_rejects_oversized_font() {
        let raw: RawConfigs = toml::from_str("[font]\nsize = 999.0\n").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(matches!(
            validate(&merged).unwrap_err(),
            crate::OzmuxConfigsError::InvalidFontSize { .. }
        ));
    }

    #[test]
    fn validate_rejects_nan_font_size() {
        let raw: RawConfigs = toml::from_str("[font]\nsize = nan\n").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(matches!(
            validate(&merged).unwrap_err(),
            crate::OzmuxConfigsError::InvalidFontSize { .. }
        ));
    }
}
