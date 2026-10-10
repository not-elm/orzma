//! The faces the terminal grid draws with, and the metrics measured from
//! them.

use crate::bundled::{
    BOLD, BOLD_ITALIC, FALLBACK_BOLD, FALLBACK_BOLD_ITALIC, FALLBACK_ITALIC, FALLBACK_REGULAR,
    ITALIC, REGULAR, SYMBOL_REGULAR,
};
use crate::error::{RendererError, RendererResult};
use crate::font::{Baseline, CellMetrics, FontFace, Thickness, Underline};
use bevy::prelude::{Deref, Resource, Vec2};
use swash::scale::ScaleContext;
use swash::{FontRef, GlyphId, tag_from_bytes};

/// The faces the terminal grid draws with: four primary faces, the bundled
/// CJK fallback for each, and the bundled symbol face.
#[derive(Resource, Clone)]
pub struct TerminalFonts {
    /// Regular weight, upright style.
    pub regular: LoadedFont,
    /// Bold weight, upright style.
    pub bold: LoadedFont,
    /// Regular weight, italic style.
    pub italic: LoadedFont,
    /// Bold weight, italic style.
    pub bold_italic: LoadedFont,
    /// Fallback regular weight, upright style (CJK / wide-character coverage).
    pub fallback_regular: LoadedFont,
    /// Fallback bold weight, upright style.
    pub fallback_bold: LoadedFont,
    /// Fallback regular weight, italic style.
    pub fallback_italic: LoadedFont,
    /// Fallback bold weight, italic style.
    pub fallback_bold_italic: LoadedFont,
    /// Symbol/dingbat fallback (e.g. checkbox marks ☐ ☑ ☒ ✔), tried after
    /// both the primary and CJK fallback miss. One face serves every
    /// `FontFace`, and it is always the bundled Noto Sans Symbols 2: no
    /// constructor takes a symbol face.
    pub symbol: LoadedFont,
    /// `.ttc` face index of the regular face. Every ttf-parser reparse of the
    /// regular face (cell metrics, em-scale) MUST use this index.
    regular_index: u32,
}

impl TerminalFonts {
    /// Constructs a `TerminalFonts` from four primary `(bytes, .ttc index)`
    /// pairs, loading each primary face at its own collection index.
    ///
    /// The fallback faces are always the bundled UDEV Gothic 35 faces and
    /// the symbol face is always the bundled Noto Sans Symbols 2, so callers
    /// supply neither.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::FontParse`] naming the face whose bytes
    /// failed to parse.
    pub fn from_faces(
        regular: (Vec<u8>, u32),
        bold: (Vec<u8>, u32),
        italic: (Vec<u8>, u32),
        bold_italic: (Vec<u8>, u32),
    ) -> RendererResult<Self> {
        let regular_index = regular.1;
        let regular = primary_font(regular.0, regular.1)?;
        let bold = primary_font(bold.0, bold.1)?;
        let italic = primary_font(italic.0, italic.1)?;
        let bold_italic = primary_font(bold_italic.0, bold_italic.1)?;
        let fallback_regular = fallback_font(FALLBACK_REGULAR)?;
        let fallback_bold = fallback_font(FALLBACK_BOLD)?;
        let fallback_italic = fallback_font(FALLBACK_ITALIC)?;
        let fallback_bold_italic = fallback_font(FALLBACK_BOLD_ITALIC)?;
        let symbol = bundled_symbol_font();
        Ok(Self {
            regular,
            bold,
            italic,
            bold_italic,
            fallback_regular,
            fallback_bold,
            fallback_italic,
            fallback_bold_italic,
            symbol,
            regular_index,
        })
    }

    /// Constructs a `TerminalFonts` from four owned TTF byte buffers, one
    /// per primary face, loading every primary face at collection index 0.
    ///
    /// The fallback faces are always the bundled UDEV Gothic 35 faces and
    /// the symbol face is always the bundled Noto Sans Symbols 2, so callers
    /// supply neither.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::FontParse`] naming the face whose bytes
    /// failed to parse.
    pub fn from_bytes(
        regular: Vec<u8>,
        bold: Vec<u8>,
        italic: Vec<u8>,
        bold_italic: Vec<u8>,
    ) -> RendererResult<Self> {
        Self::from_faces((regular, 0), (bold, 0), (italic, 0), (bold_italic, 0))
    }

    /// Returns the primary face matching `face`.
    pub fn choice(&self, face: &FontFace) -> &LoadedFont {
        match face {
            FontFace::Regular => &self.regular,
            FontFace::Bold => &self.bold,
            FontFace::Italic => &self.italic,
            FontFace::BoldItalic => &self.bold_italic,
        }
    }

    /// Returns the fallback face matching `face`.
    ///
    /// The fallback faces serve glyph lookup alone.
    pub fn fallback_choice(&self, face: &FontFace) -> &LoadedFont {
        match face {
            FontFace::Regular => &self.fallback_regular,
            FontFace::Bold => &self.fallback_bold,
            FontFace::Italic => &self.fallback_italic,
            FontFace::BoldItalic => &self.fallback_bold_italic,
        }
    }

    /// Returns full pixel metrics for the regular face at the requested
    /// physical pixel size.
    pub fn cell_metrics_px(&self, phys_size_px: u16) -> CellMetrics {
        let font = &self.regular.0;
        let px = f32::from(phys_size_px);
        let metrics = font.metrics(&[]);
        let upem = f32::from(metrics.units_per_em);
        let to_px = |units: f32| units * px / upem;
        let advance_phys = to_px(
            font.glyph_metrics(&[])
                .advance_width(font.charmap().map('0')),
        );
        let ascent_phys = to_px(metrics.ascent);
        let line_height_phys = to_px(metrics.ascent + metrics.descent + metrics.leading);
        let underline = if font.table(tag_from_bytes(b"post")).is_some() {
            Underline {
                position: to_px(metrics.underline_offset),
                thickness: Thickness::new(to_px(metrics.stroke_size)),
            }
        } else {
            Underline {
                position: -ascent_phys * 0.07,
                thickness: Thickness::new(ascent_phys / 14.0),
            }
        };
        let cell_size = Vec2::new(advance_phys, line_height_phys)
            .floor()
            .max(Vec2::ONE);
        let mut context = ScaleContext::new();
        let max_overflow = [&self.regular, &self.italic, &self.bold, &self.bold_italic]
            .into_iter()
            .map(|font| max_ascii_overflow(&mut context, font.0, px, cell_size.x))
            .fold(0.0_f32, f32::max);

        CellMetrics {
            cell_size,
            baseline: Baseline::new(ascent_phys),
            underline,
            max_overflow,
        }
    }
}

impl Default for TerminalFonts {
    fn default() -> Self {
        Self {
            regular: LoadedFont::bundled(REGULAR),
            bold: LoadedFont::bundled(BOLD),
            italic: LoadedFont::bundled(ITALIC),
            bold_italic: LoadedFont::bundled(BOLD_ITALIC),
            fallback_regular: LoadedFont::bundled(FALLBACK_REGULAR),
            fallback_bold: LoadedFont::bundled(FALLBACK_BOLD),
            fallback_italic: LoadedFont::bundled(FALLBACK_ITALIC),
            fallback_bold_italic: LoadedFont::bundled(FALLBACK_BOLD_ITALIC),
            symbol: LoadedFont::bundled(SYMBOL_REGULAR),
            regular_index: 0,
        }
    }
}

#[derive(Clone, Deref)]
pub struct LoadedFont(FontRef<'static>);

impl LoadedFont {
    pub fn new(bytes: &'static [u8], index: usize) -> RendererResult<Self> {
        let font = FontRef::from_index(bytes, index).ok_or(RendererError::FontParse)?;
        Ok(Self(font))
    }

    pub fn bundled(bytes: &'static [u8]) -> Self {
        Self::new(bytes, 0).expect("a bundled font")
    }

    pub fn glyph_id(&self, code_point: char) -> GlyphId {
        self.0.charmap().map(code_point)
    }
}

/// The furthest any printable ASCII glyph of `face` reaches past
/// `cell_w_phys_floor` at `px` px per em, measured to the outline's right
/// edge rounded up to a whole pixel; 0 when none reaches past it.
fn max_ascii_overflow(
    context: &mut ScaleContext,
    font: FontRef<'static>,
    px: f32,
    cell_w_phys_floor: f32,
) -> f32 {
    let charmap = font.charmap();
    let mut scaler = context.builder(font).size(px).hint(false).build();
    (0x20u8..=0x7E)
        .map(|byte| charmap.map(char::from(byte)))
        .filter(|glyph_id| *glyph_id != 0)
        .filter_map(|glyph_id| scaler.scale_outline(glyph_id))
        .map(|outline| outline.bounds().max.x.ceil() - cell_w_phys_floor)
        .fold(0.0_f32, f32::max)
}

/// Loads the bundled symbol/dingbat fallback face (Noto Sans Symbols 2).
fn bundled_symbol_font() -> LoadedFont {
    LoadedFont::bundled(SYMBOL_REGULAR)
}

/// Builds one primary `Font` from owned bytes at a `.ttc` face index.
///
/// The bytes are leaked, so they stay allocated for the rest of the process.
fn primary_font(bytes: Vec<u8>, index: u32) -> RendererResult<LoadedFont> {
    LoadedFont::new(Box::leak(bytes.into_boxed_slice()), index as usize)
}

/// Loads one bundled fallback face at face index 0.
fn fallback_font(bytes: &'static [u8]) -> RendererResult<LoadedFont> {
    LoadedFont::new(bytes, 0)
}
