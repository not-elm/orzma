//! Font configuration: the `[font]` section.

use serde::Deserialize;

const DEFAULT_SIZE: f32 = 11.25;

/// Fully-resolved font configuration for the terminal grid.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    /// Terminal font size in logical (CSS) pixels, scaled by the display's
    /// `scale_factor` to device pixels — Alacritty's model (not literal
    /// typographic points; no 96/72 conversion is applied).
    pub size: f32,
    /// Base font-family name resolved against the system font database. `None`
    /// uses the bundled JetBrains Mono Nerd Font.
    pub family: Option<String>,
    /// Optional family-name override for the bold face; `None` derives it from
    /// `family`.
    pub bold_family: Option<String>,
    /// Optional family-name override for the italic face; `None` derives it from
    /// `family`.
    pub italic_family: Option<String>,
    /// Optional family-name override for the bold-italic face; `None` derives it
    /// from `family`.
    pub bold_italic_family: Option<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            family: None,
            bold_family: None,
            italic_family: None,
            bold_italic_family: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_matches_alacritty() {
        assert_eq!(FontConfig::default().size, 11.25);
    }

    #[test]
    fn empty_is_default() {
        let f: FontConfig = toml::from_str("").unwrap();
        assert_eq!(f, FontConfig::default());
    }

    #[test]
    fn parses_family_and_per_face_overrides() {
        let f: FontConfig = toml::from_str(
            "size = 14.0\nfamily = \"JetBrains Mono\"\nitalic_family = \"Cascadia Code\"",
        )
        .unwrap();
        assert_eq!(f.size, 14.0);
        assert_eq!(f.family.as_deref(), Some("JetBrains Mono"));
        assert_eq!(f.italic_family.as_deref(), Some("Cascadia Code"));
        assert_eq!(f.bold_family, None);
        assert_eq!(f.bold_italic_family, None);
    }

    #[test]
    fn size_only_leaves_families_none() {
        let f: FontConfig = toml::from_str("size = 18.0").unwrap();
        assert_eq!(f.size, 18.0);
        assert_eq!(f.family, None);
    }

    #[test]
    fn old_path_keys_are_ignored_not_families() {
        // Clean break: the removed `normal`/`bold` path keys no longer populate
        // any face — they are unknown keys and silently ignored.
        let f: FontConfig = toml::from_str("normal = \"/x.ttf\"\nbold = \"/y.ttf\"").unwrap();
        assert_eq!(f.family, None);
        assert_eq!(f.bold_family, None);
    }
}
