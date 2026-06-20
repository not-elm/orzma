//! Font configuration: the `[font]` section.

use serde::Deserialize;

const DEFAULT_SIZE: f32 = 11.25;

/// Fully-resolved font configuration for the terminal grid.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    /// Terminal font size in points, matching Alacritty.
    pub size: f32,
    /// Absolute or `~`-prefixed path to the regular-face TTF (Bevy GUI only).
    pub normal: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-face TTF (Bevy GUI only).
    pub bold: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the italic-face TTF (Bevy GUI only).
    pub italic: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-italic-face TTF (Bevy GUI only).
    pub bold_italic: Option<std::path::PathBuf>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            normal: None,
            bold: None,
            italic: None,
            bold_italic: None,
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
    fn parses_flat_paths() {
        let f: FontConfig =
            toml::from_str("size = 14.0\nnormal = \"/abs/Regular.ttf\"\nbold = \"/abs/Bold.ttf\"")
                .unwrap();
        assert_eq!(f.size, 14.0);
        assert_eq!(
            f.normal.as_deref(),
            Some(std::path::Path::new("/abs/Regular.ttf"))
        );
        assert_eq!(
            f.bold.as_deref(),
            Some(std::path::Path::new("/abs/Bold.ttf"))
        );
        assert_eq!(f.italic, None);
        assert_eq!(f.bold_italic, None);
    }

    #[test]
    fn size_override_keeps_paths_none() {
        let f: FontConfig = toml::from_str("size = 18.0").unwrap();
        assert_eq!(f.size, 18.0);
        assert_eq!(f.normal, None);
    }

    #[test]
    fn old_nested_table_form_is_rejected() {
        let err = toml::from_str::<FontConfig>("[normal]\npath = \"/x.ttf\"").is_err();
        assert!(
            err,
            "old nested [font.normal] path= form must fail to parse"
        );
    }
}
