//! Font configuration: Alacritty-compatible `[font]` section.

use serde::{Deserialize, Serialize};

const DEFAULT_SIZE: f32 = 11.25;

/// Fully-resolved font configuration for the terminal grid.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FontConfig {
    /// Terminal font size in points, matching Alacritty. The frontend
    /// converts points to CSS pixels at render time.
    pub size: f32,
    /// Absolute or `~`-prefixed path to the regular-face TTF
    /// (Bevy GUI only).
    // NOTE: skip_serializing keeps the user's local filesystem path out
    // of any serialized form of this config (an information leak if it
    // were ever exposed beyond the local process).
    #[serde(skip_serializing)]
    pub normal_path: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-face TTF (Bevy GUI only).
    #[serde(skip_serializing)]
    pub bold_path: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the italic-face TTF (Bevy GUI only).
    #[serde(skip_serializing)]
    pub italic_path: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-italic-face TTF (Bevy GUI only).
    #[serde(skip_serializing)]
    pub bold_italic_path: Option<std::path::PathBuf>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
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
    /// Resolves the patch against `base`. Each face's path override is read
    /// from its own `FacePatch` only; there is no inheritance from
    /// `normal_path`. See the design doc (Approach summary item 6) for the
    /// rationale.
    pub(crate) fn apply_to(self, base: FontConfig) -> FontConfig {
        let normal_path = self.normal.and_then(|p| p.path).or(base.normal_path);
        let bold_path = self.bold.and_then(|p| p.path).or(base.bold_path);
        let italic_path = self.italic.and_then(|p| p.path).or(base.italic_path);
        let bold_italic_path = self
            .bold_italic
            .and_then(|p| p.path)
            .or(base.bold_italic_path);

        FontConfig {
            size: self.size.unwrap_or(base.size),
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
    fn face_patch_ignores_alacritty_only_keys() {
        let patch: FontPatch = toml::from_str(
            r#"
            size = 14.0
            [normal]
            family = "Hack Nerd Font"
            style = "Regular"
            path = "/abs/regular.ttf"
        "#,
        )
        .unwrap();
        assert_eq!(patch.size, Some(14.0));
        let normal = patch.normal.unwrap();
        assert_eq!(
            normal.path.as_deref(),
            Some(std::path::Path::new("/abs/regular.ttf")),
        );
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
                path: Some(std::path::PathBuf::from("/r.ttf")),
            }),
            bold: Some(FacePatch {
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
