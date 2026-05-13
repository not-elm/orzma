//! Theme tokens and patching for per-field merge against defaults.

use serde::{Deserialize, Serialize};

/// Fully-resolved theme: five semantic color tokens that ozmux exposes to the UI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// `--color-background` token.
    pub background: String,
    /// `--color-foreground` token.
    pub foreground: String,
    /// `--color-accent` token.
    pub accent: String,
    /// `--color-border` token.
    pub border: String,
    /// `--color-destructive` token.
    pub destructive: String,
}

/// Per-field-optional view of `Theme` used for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
pub struct ThemePatch {
    /// Optional override for `Theme::background`.
    pub background: Option<String>,
    /// Optional override for `Theme::foreground`.
    pub foreground: Option<String>,
    /// Optional override for `Theme::accent`.
    pub accent: Option<String>,
    /// Optional override for `Theme::border`.
    pub border: Option<String>,
    /// Optional override for `Theme::destructive`.
    pub destructive: Option<String>,
}

impl ThemePatch {
    /// Applies any `Some` fields onto `base` and returns the merged `Theme`.
    pub fn apply_to(self, mut base: Theme) -> Theme {
        if let Some(v) = self.background {
            base.background = v;
        }
        if let Some(v) = self.foreground {
            base.foreground = v;
        }
        if let Some(v) = self.accent {
            base.accent = v;
        }
        if let Some(v) = self.border {
            base.border = v;
        }
        if let Some(v) = self.destructive {
            base.destructive = v;
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_theme() -> Theme {
        Theme {
            background: "#000000".into(),
            foreground: "#ffffff".into(),
            accent: "#111111".into(),
            border: "#222222".into(),
            destructive: "#ff0000".into(),
        }
    }

    #[test]
    fn theme_patch_apply_overrides_only_set_fields() {
        let patch = ThemePatch {
            accent: Some("#abcdef".into()),
            ..Default::default()
        };
        let merged = patch.apply_to(sample_theme());
        assert_eq!(merged.accent, "#abcdef");
        assert_eq!(merged.background, "#000000");
        assert_eq!(merged.foreground, "#ffffff");
        assert_eq!(merged.border, "#222222");
        assert_eq!(merged.destructive, "#ff0000");
    }

    #[test]
    fn theme_patch_empty_returns_base() {
        let patch = ThemePatch::default();
        let merged = patch.apply_to(sample_theme());
        assert_eq!(merged, sample_theme());
    }

    #[test]
    fn theme_patch_deserializes_partial_toml() {
        let raw = r##"accent = "#abcdef""##;
        let patch: ThemePatch = toml::from_str(raw).unwrap();
        assert_eq!(patch.accent.as_deref(), Some("#abcdef"));
        assert!(patch.background.is_none());
    }
}
