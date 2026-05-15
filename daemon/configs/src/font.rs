//! Font configuration: Alacritty-compatible `[font]` section.

use serde::{Deserialize, Serialize};

const DEFAULT_FAMILY: &str = "ui-monospace, \"SF Mono\", Menlo, Consolas, monospace";
const DEFAULT_SIZE: f32 = 16.0;

/// Fully-resolved font configuration for the terminal grid.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FontConfig {
    /// Terminal font size in CSS pixels (deliberate deviation from
    /// Alacritty's points — the value flows straight into a CSS `font-size`).
    pub size: f32,
    /// Font family for normal-weight cells.
    pub normal_family: String,
    /// Font family for bold cells.
    pub bold_family: String,
    /// Font family for italic cells.
    pub italic_family: String,
    /// Font family for bold + italic cells.
    pub bold_italic_family: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            normal_family: DEFAULT_FAMILY.into(),
            bold_family: DEFAULT_FAMILY.into(),
            italic_family: DEFAULT_FAMILY.into(),
            bold_italic_family: DEFAULT_FAMILY.into(),
        }
    }
}

/// Per-face TOML patch — the body of `[font.normal]`, `[font.bold]`, etc.
#[derive(Deserialize, Default, Clone, Debug)]
pub(crate) struct FacePatch {
    /// Family-name override for this face.
    pub family: Option<String>,
    /// Alacritty's `style` key — accepted for format compatibility and
    /// discarded (the web renderer uses CSS weight/style).
    pub style: Option<String>,
}

/// Per-field-optional view of the `[font]` section for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
pub(crate) struct FontPatch {
    /// Optional `[font].size` override.
    pub size: Option<f32>,
    /// Optional `[font.normal]` face.
    pub normal: Option<FacePatch>,
    /// Optional `[font.bold]` face.
    pub bold: Option<FacePatch>,
    /// Optional `[font.italic]` face.
    pub italic: Option<FacePatch>,
    /// Optional `[font.bold_italic]` face.
    pub bold_italic: Option<FacePatch>,
}

impl FontPatch {
    /// Resolves the patch against `base`. Applies Alacritty's fallback:
    /// when a bold/italic/bold_italic `family` is unset, it defaults to the
    /// resolved `normal` family rather than the base value.
    pub fn apply_to(self, base: FontConfig) -> FontConfig {
        let face_family = |f: Option<FacePatch>| f.and_then(|p| p.family);
        let normal_family = face_family(self.normal).unwrap_or(base.normal_family);
        let bold_family = face_family(self.bold).unwrap_or_else(|| normal_family.clone());
        let italic_family = face_family(self.italic).unwrap_or_else(|| normal_family.clone());
        let bold_italic_family =
            face_family(self.bold_italic).unwrap_or_else(|| normal_family.clone());
        FontConfig {
            size: self.size.unwrap_or(base.size),
            normal_family,
            bold_family,
            italic_family,
            bold_italic_family,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_patch_returns_base() {
        let merged = FontPatch::default().apply_to(FontConfig::default());
        assert_eq!(merged, FontConfig::default());
    }

    #[test]
    fn normal_only_falls_back_to_normal_for_bold_and_italic() {
        let patch = FontPatch {
            normal: Some(FacePatch {
                family: Some("Hack Nerd Font".into()),
                style: None,
            }),
            ..Default::default()
        };
        let merged = patch.apply_to(FontConfig::default());
        assert_eq!(merged.normal_family, "Hack Nerd Font");
        assert_eq!(merged.bold_family, "Hack Nerd Font");
        assert_eq!(merged.italic_family, "Hack Nerd Font");
        assert_eq!(merged.bold_italic_family, "Hack Nerd Font");
    }

    #[test]
    fn explicit_bold_family_overrides_fallback() {
        let patch = FontPatch {
            normal: Some(FacePatch {
                family: Some("Hack Nerd Font".into()),
                style: None,
            }),
            bold: Some(FacePatch {
                family: Some("JetBrainsMono Nerd Font".into()),
                style: None,
            }),
            ..Default::default()
        };
        let merged = patch.apply_to(FontConfig::default());
        assert_eq!(merged.bold_family, "JetBrainsMono Nerd Font");
        assert_eq!(merged.italic_family, "Hack Nerd Font");
    }

    #[test]
    fn size_override_applies() {
        let patch = FontPatch {
            size: Some(18.0),
            ..Default::default()
        };
        let merged = patch.apply_to(FontConfig::default());
        assert_eq!(merged.size, 18.0);
    }

    #[test]
    fn face_patch_deserializes_and_ignores_style() {
        let patch: FontPatch = toml::from_str(
            r#"
            size = 14.0
            [normal]
            family = "Hack Nerd Font"
            style = "Regular"
        "#,
        )
        .unwrap();
        assert_eq!(patch.size, Some(14.0));
        let normal = patch.normal.unwrap();
        assert_eq!(normal.family.as_deref(), Some("Hack Nerd Font"));
        assert_eq!(normal.style.as_deref(), Some("Regular"));
    }
}
