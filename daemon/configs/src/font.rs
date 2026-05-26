//! Font configuration: Alacritty-compatible `[font]` section.

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
const DEFAULT_FAMILY: &str = "Menlo";
#[cfg(target_os = "windows")]
const DEFAULT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const DEFAULT_FAMILY: &str = "monospace";
const DEFAULT_SIZE: f32 = 11.25;

/// Fully-resolved font configuration for the terminal grid.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FontConfig {
    /// Terminal font size in points, matching Alacritty. The frontend
    /// converts points to CSS pixels at render time.
    pub size: f32,
    /// Font family for normal-weight cells.
    pub normal_family: String,
    /// Font family for bold cells.
    pub bold_family: String,
    /// Font family for italic cells.
    pub italic_family: String,
    /// Font family for bold + italic cells.
    pub bold_italic_family: String,
    /// Absolute or `~`-prefixed path to the regular-face TTF.
    /// Consumed by the Bevy GUI only; the web frontend ignores this and
    /// uses `normal_family` for CSS lookup.
    pub normal_path: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-face TTF (Bevy GUI only).
    pub bold_path: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the italic-face TTF (Bevy GUI only).
    pub italic_path: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-italic-face TTF (Bevy GUI only).
    pub bold_italic_path: Option<std::path::PathBuf>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            normal_family: DEFAULT_FAMILY.into(),
            bold_family: DEFAULT_FAMILY.into(),
            italic_family: DEFAULT_FAMILY.into(),
            bold_italic_family: DEFAULT_FAMILY.into(),
            normal_path: None,
            bold_path: None,
            italic_path: None,
            bold_italic_path: None,
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
    // NOTE: read only by `#[cfg(test)]` code; `#[allow]` (not `#[expect]`)
    // because the lint fires under `cargo build` but not `cargo test`.
    #[allow(
        dead_code,
        reason = "accepted for Alacritty [font.*] compatibility, never applied"
    )]
    pub style: Option<String>,
    /// TTF path override (Bevy GUI only). Read independently per-face;
    /// no inheritance from the normal face.
    pub path: Option<std::path::PathBuf>,
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
    /// resolved `normal` family rather than the base value. Path overrides
    /// (`*_path`) follow a DIFFERENT rule: each face's path is read from
    /// its own FacePatch only; no inheritance from `normal_path`. See the
    /// design doc (Approach summary item 6) for the rationale.
    pub fn apply_to(self, base: FontConfig) -> FontConfig {
        let normal_face = self.normal;
        let bold_face = self.bold;
        let italic_face = self.italic;
        let bold_italic_face = self.bold_italic;

        let normal_family = normal_face
            .as_ref()
            .and_then(|p| p.family.clone())
            .unwrap_or(base.normal_family);
        let bold_family = bold_face
            .as_ref()
            .and_then(|p| p.family.clone())
            .unwrap_or_else(|| normal_family.clone());
        let italic_family = italic_face
            .as_ref()
            .and_then(|p| p.family.clone())
            .unwrap_or_else(|| normal_family.clone());
        let bold_italic_family = bold_italic_face
            .as_ref()
            .and_then(|p| p.family.clone())
            .unwrap_or_else(|| normal_family.clone());

        let normal_path = normal_face.and_then(|p| p.path).or(base.normal_path);
        let bold_path = bold_face.and_then(|p| p.path).or(base.bold_path);
        let italic_path = italic_face.and_then(|p| p.path).or(base.italic_path);
        let bold_italic_path = bold_italic_face
            .and_then(|p| p.path)
            .or(base.bold_italic_path);

        FontConfig {
            size: self.size.unwrap_or(base.size),
            normal_family,
            bold_family,
            italic_family,
            bold_italic_family,
            normal_path,
            bold_path,
            italic_path,
            bold_italic_path,
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
    fn default_matches_alacritty() {
        let d = FontConfig::default();
        assert_eq!(d.size, 11.25);
        #[cfg(target_os = "macos")]
        assert_eq!(d.normal_family, "Menlo");
        #[cfg(target_os = "windows")]
        assert_eq!(d.normal_family, "Consolas");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(d.normal_family, "monospace");
        assert_eq!(d.bold_family, d.normal_family);
        assert_eq!(d.italic_family, d.normal_family);
        assert_eq!(d.bold_italic_family, d.normal_family);
    }

    #[test]
    fn normal_only_falls_back_to_normal_for_bold_and_italic() {
        let patch = FontPatch {
            normal: Some(FacePatch {
                family: Some("Hack Nerd Font".into()),
                style: None,
                path: None,
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
                path: None,
            }),
            bold: Some(FacePatch {
                family: Some("JetBrainsMono Nerd Font".into()),
                style: None,
                path: None,
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

    #[test]
    fn font_config_default_has_no_paths_set() {
        let d = FontConfig::default();
        assert_eq!(d.normal_path, None);
        assert_eq!(d.bold_path, None);
        assert_eq!(d.italic_path, None);
        assert_eq!(d.bold_italic_path, None);
    }

    #[test]
    fn font_patch_parses_per_face_path() {
        let patch: FontPatch = toml::from_str(
            r#"
            [normal]
            path = "/abs/regular.ttf"
            [bold]
            path = "/abs/bold.ttf"
        "#,
        )
        .unwrap();
        assert_eq!(
            patch.normal.as_ref().and_then(|p| p.path.as_deref()),
            Some(std::path::Path::new("/abs/regular.ttf")),
        );
        assert_eq!(
            patch.bold.as_ref().and_then(|p| p.path.as_deref()),
            Some(std::path::Path::new("/abs/bold.ttf")),
        );
    }

    #[test]
    fn apply_to_propagates_paths_without_inheritance() {
        let patch = FontPatch {
            normal: Some(FacePatch {
                family: None,
                style: None,
                path: Some(std::path::PathBuf::from("/abs/regular.ttf")),
            }),
            ..Default::default()
        };
        let merged = patch.apply_to(FontConfig::default());
        assert_eq!(
            merged.normal_path,
            Some(std::path::PathBuf::from("/abs/regular.ttf"))
        );
        assert_eq!(
            merged.bold_path, None,
            "bold_path must NOT inherit from normal_path"
        );
        assert_eq!(
            merged.italic_path, None,
            "italic_path must NOT inherit from normal_path"
        );
        assert_eq!(
            merged.bold_italic_path, None,
            "bold_italic_path must NOT inherit from normal_path"
        );
    }

    #[test]
    fn apply_to_propagates_each_face_independently() {
        let patch = FontPatch {
            normal: Some(FacePatch {
                family: None,
                style: None,
                path: Some(std::path::PathBuf::from("/r.ttf")),
            }),
            bold: Some(FacePatch {
                family: None,
                style: None,
                path: Some(std::path::PathBuf::from("/b.ttf")),
            }),
            ..Default::default()
        };
        let merged = patch.apply_to(FontConfig::default());
        assert_eq!(merged.normal_path, Some(std::path::PathBuf::from("/r.ttf")));
        assert_eq!(merged.bold_path, Some(std::path::PathBuf::from("/b.ttf")));
        assert_eq!(merged.italic_path, None);
        assert_eq!(merged.bold_italic_path, None);
    }
}
