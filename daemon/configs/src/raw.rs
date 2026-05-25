//! TOML-deserialization layer for `OzmuxConfigs`. Holds optional sections
//! and merges them onto a baseline `OzmuxConfigs::default()`.

use crate::OzmuxConfigs;
use crate::OzmuxConfigsError;
use crate::OzmuxConfigsResult;
use crate::browser::BrowserPatch;
use crate::font::FontPatch;
use crate::mouse::MousePatch;
use crate::shortcuts::Bindings;
use crate::theme::ThemePatch;
use serde::Deserialize;

/// Top-level TOML shape: every section is optional.
#[derive(Deserialize, Default)]
pub(crate) struct RawConfigs {
    pub(crate) shortcuts: Option<RawShortcuts>,
    pub(crate) theme: Option<ThemePatch>,
    pub(crate) font: Option<FontPatch>,
    pub(crate) browser: Option<BrowserPatch>,
    pub(crate) mouse: Option<MousePatch>,
}

/// `[shortcuts]` shape with each subfield optional.
#[derive(Deserialize, Default)]
pub(crate) struct RawShortcuts {
    pub(crate) bindings: Option<Bindings>,
}

impl RawConfigs {
    /// Applies any populated fields onto `base` and returns the merged result.
    /// Within the `shortcuts` section, `prefix` and `bindings` are independently
    /// optional; `bindings` is full-replace. The `theme` and `font` sections use
    /// their respective `Patch::apply_to` for per-field merge.
    pub(crate) fn apply_to(self, mut base: OzmuxConfigs) -> OzmuxConfigs {
        if let Some(raw) = self.shortcuts
            && let Some(bindings) = raw.bindings
        {
            base.shortcuts.bindings = bindings;
        }
        if let Some(patch) = self.theme {
            base.theme = patch.apply_to(base.theme);
        }
        if let Some(patch) = self.font {
            base.font = patch.apply_to(base.font);
        }
        if let Some(patch) = self.browser {
            base.browser = patch.apply_to(base.browser);
        }
        if let Some(patch) = self.mouse {
            base.mouse = patch.apply_to(base.mouse);
        }
        base
    }
}

/// Walks `configs.shortcuts.bindings` and rejects duplicate chords.
pub(crate) fn validate(configs: &OzmuxConfigs) -> OzmuxConfigsResult {
    if let Err(dupes) = configs.shortcuts.bindings.validate_no_conflicts() {
        // Map the new Vec<DuplicateChord> into the legacy single-variant
        // error for the transitional state (Task 3.1 introduces the
        // proper DuplicateChords(Vec<...>) variant).
        if let Some(first) = dupes.into_iter().next() {
            return Err(OzmuxConfigsError::DuplicateBinding { chord: first.chord });
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
    fn browser_section_overrides_search_template() {
        let raw: RawConfigs = toml::from_str(
            r#"
            [browser]
            search_template = "https://www.google.com/search?q={query}"
        "#,
        )
        .unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(
            merged.browser.search_template,
            "https://www.google.com/search?q={query}"
        );
    }

    #[test]
    fn absent_browser_section_keeps_default_duckduckgo() {
        let raw = RawConfigs::default();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.browser, crate::browser::BrowserConfig::default());
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
