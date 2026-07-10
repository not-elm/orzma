//! Font configuration: the `[font]` section.

mod style;

use serde::Deserialize;
pub use style::{FontSlant, FontStyleSpec, InvalidFontStyleToken};

const DEFAULT_SIZE: f32 = 11.25;

/// One face's font configuration: a family name and a style string, both
/// optional. Omitted `family` inherits `normal`'s; omitted `style` uses the
/// face's canonical default (applied in `src/font`).
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct FontFaceConfig {
    /// Font-family name resolved against the system font database.
    pub family: Option<String>,
    /// Alacritty-style style string (e.g. `"Bold"`, `"SemiBold Italic"`).
    pub style: Option<String>,
}

/// The `[font]` section: a size plus the four terminal faces.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    /// Terminal font size in logical (CSS) pixels, scaled by the display's
    /// `scale_factor` to device pixels — Alacritty's model (not literal
    /// typographic points; no 96/72 conversion is applied).
    pub size: f32,
    /// The regular face; its `family` is the base every other face inherits.
    pub normal: FontFaceConfig,
    /// The bold face; `family`/`style` default from `normal` / Bold.
    pub bold: FontFaceConfig,
    /// The italic face; `family`/`style` default from `normal` / Italic.
    pub italic: FontFaceConfig,
    /// The bold-italic face; `family`/`style` default from `normal` / Bold Italic.
    pub bold_italic: FontFaceConfig,
    /// The UI-chrome face (window bar, prompts, indicators). `family` and
    /// `style` each inherit from `normal` when omitted (not from this face's
    /// own defaults); resolution and rendering live in `src/font`.
    pub ui: FontFaceConfig,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            normal: FontFaceConfig::default(),
            bold: FontFaceConfig::default(),
            italic: FontFaceConfig::default(),
            bold_italic: FontFaceConfig::default(),
            ui: FontFaceConfig::default(),
        }
    }
}

impl FontConfig {
    /// Face labels whose `style` is set but whose effective family (own or
    /// inherited from `normal`) is absent — so the bundled default is used and
    /// `style` is silently ignored (D5/D9). Used to emit a load-time warning.
    pub fn faces_with_ignored_style(&self) -> Vec<&'static str> {
        let base_present = self.normal.family.is_some();
        self.faces()
            .into_iter()
            .filter(|(_, face)| face.style.is_some() && face.family.is_none() && !base_present)
            .map(|(label, _)| label)
            .collect()
    }

    /// Whether no face configures a font family — every face's `family` is
    /// absent, so the bundled default font is used.
    #[inline]
    pub fn has_no_configured_family(&self) -> bool {
        self.faces().iter().all(|(_, c)| c.family.is_none())
    }

    /// The four terminal faces paired with their `[font]` key labels, in fixed
    /// order — the single source of truth for the face set.
    pub const fn faces(&self) -> [(&'static str, &FontFaceConfig); 4] {
        [
            ("normal", &self.normal),
            ("bold", &self.bold),
            ("italic", &self.italic),
            ("bold_italic", &self.bold_italic),
        ]
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
    fn parses_nested_face_tables() {
        let f: FontConfig = toml::from_str(
            "size = 14.0\n[normal]\nfamily = \"JetBrains Mono\"\nstyle = \"Medium\"\n[italic]\nfamily = \"Cascadia Code\"",
        )
        .unwrap();
        assert_eq!(f.size, 14.0);
        assert_eq!(f.normal.family.as_deref(), Some("JetBrains Mono"));
        assert_eq!(f.normal.style.as_deref(), Some("Medium"));
        assert_eq!(f.italic.family.as_deref(), Some("Cascadia Code"));
        assert_eq!(f.italic.style, None);
        assert_eq!(f.bold, FontFaceConfig::default());
    }

    #[test]
    fn parses_inline_face_tables() {
        let f: FontConfig =
            toml::from_str("normal = { family = \"Iosevka\", style = \"Bold\" }").unwrap();
        assert_eq!(f.normal.family.as_deref(), Some("Iosevka"));
        assert_eq!(f.normal.style.as_deref(), Some("Bold"));
    }

    #[test]
    fn unknown_face_field_is_rejected() {
        assert!(
            toml::from_str::<FontConfig>("[normal]\nfamilly = \"x\"").is_err(),
            "a typo'd face field must error under deny_unknown_fields"
        );
    }

    #[test]
    fn unknown_top_level_font_field_is_rejected() {
        assert!(toml::from_str::<FontConfig>("weight = 700").is_err());
    }

    #[test]
    fn faces_with_ignored_style_flags_style_without_family() {
        let f: FontConfig = toml::from_str("[bold]\nstyle = \"Bold\"").unwrap();
        assert_eq!(f.faces_with_ignored_style(), vec!["bold"]);

        let inherited: FontConfig =
            toml::from_str("[normal]\nfamily = \"Iosevka\"\n[bold]\nstyle = \"Bold\"").unwrap();
        assert!(inherited.faces_with_ignored_style().is_empty());
    }

    #[test]
    fn parses_ui_face_inline_and_section() {
        let inline: FontConfig =
            toml::from_str("ui = { family = \"Inter\", style = \"Medium\" }").unwrap();
        assert_eq!(inline.ui.family.as_deref(), Some("Inter"));
        assert_eq!(inline.ui.style.as_deref(), Some("Medium"));

        let section: FontConfig =
            toml::from_str("[ui]\nfamily = \"Inter\"\nstyle = \"Bold Italic\"").unwrap();
        assert_eq!(section.ui.family.as_deref(), Some("Inter"));
        assert_eq!(section.ui.style.as_deref(), Some("Bold Italic"));
    }

    #[test]
    fn ui_defaults_to_empty_face() {
        let f: FontConfig = toml::from_str("").unwrap();
        assert_eq!(f.ui, FontFaceConfig::default());
    }
}
