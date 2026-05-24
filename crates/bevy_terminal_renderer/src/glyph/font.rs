use ab_glyph::{Font, FontArc, ScaleFont};
use bevy::prelude::*;

#[derive(Default)]
pub struct TerminalFontPlugin;

impl Plugin for TerminalFontPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TerminalFonts::default());
    }
}

#[derive(Resource, Clone)]
pub struct TerminalFonts {
    /// Regular weight, upright style.
    pub regular: FontArc,
    /// Bold weight, upright style.
    pub bold: FontArc,
    /// Regular weight, italic style.
    pub italic: FontArc,
    /// Bold weight, italic style.
    pub bold_italic: FontArc,
}

impl TerminalFonts {
    pub fn choice(&self, face: &FontFace) -> &FontArc {
        match face {
            FontFace::Regular => &self.regular,
            FontFace::Bold => &self.bold,
            FontFace::Italic => &self.italic,
            FontFace::BoldItalic => &self.bold_italic,
        }
    }

    /// Returns the typographic ascent of the regular face at the requested
    /// pixel size. Positive value in pixels above the baseline.
    #[inline]
    pub fn ascent_px(&self, size_px: u16) -> f32 {
        self.regular
            .as_scaled(ab_glyph::PxScale::from(size_px as f32))
            .ascent()
    }
}

impl Default for TerminalFonts {
    fn default() -> Self {
        Self {
            regular: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-Regular.ttf"))
                .expect("JetBrainsMode-Regular load"),
            bold: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-Bold.ttf"))
                .expect("JetBrainsMode-Regular load"),
            italic: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-Italic.ttf"))
                .expect("JetBrainsMode-Regular load"),
            bold_italic: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-BoldItalic.ttf"))
                .expect("JetBrainsMode-Regular load"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub face: FontFace,
    pub codepoint: u32,
    pub size_px: u16,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum FontFace {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontFace {
    pub fn from_style(style: u16) -> Self {
        const BOLD: u16 = 1;
        const ITALIC: u16 = 2;
        let bold = (style & BOLD) != 0;
        let italic = (style & ITALIC) != 0;
        match (bold, italic) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }
}
