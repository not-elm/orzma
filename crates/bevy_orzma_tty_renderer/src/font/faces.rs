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
    /// Returns [`RendererError::FontParse`] when a face's bytes are not a
    /// font or hold no face at its `.ttc` index. The error does not say
    /// which face failed.
    pub fn from_faces(
        regular: (Vec<u8>, u32),
        bold: (Vec<u8>, u32),
        italic: (Vec<u8>, u32),
        bold_italic: (Vec<u8>, u32),
    ) -> RendererResult<Self> {
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
    /// Returns [`RendererError::FontParse`] when a face's bytes are not a
    /// font. The error does not say which face failed.
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

    pub fn font_ref(&self) -> FontRef<'static> {
        self.0
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

/// Builds one primary [`LoadedFont`] from owned bytes at a `.ttc` face index.
///
/// The bytes are leaked, so they stay allocated for the rest of the process.
fn primary_font(bytes: Vec<u8>, index: u32) -> RendererResult<LoadedFont> {
    LoadedFont::new(Box::leak(bytes.into_boxed_slice()), index as usize)
}

/// Loads one bundled fallback face at face index 0.
fn fallback_font(bytes: &'static [u8]) -> RendererResult<LoadedFont> {
    LoadedFont::new(bytes, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    /// Asserts that `from_faces` with every index at 0 loads the same
    /// regular face, and measures the same cell, as `from_bytes`.
    ///
    /// Case: a configured font file is a plain `.ttf`, so the loader hands
    /// it over at collection index 0.
    #[test]
    fn from_faces_index_zero_matches_from_bytes() {
        let via_bytes = TerminalFonts::from_bytes(
            bundled::REGULAR.to_vec(),
            bundled::BOLD.to_vec(),
            bundled::ITALIC.to_vec(),
            bundled::BOLD_ITALIC.to_vec(),
        )
        .expect("from_bytes");
        let via_faces = TerminalFonts::from_faces(
            (bundled::REGULAR.to_vec(), 0),
            (bundled::BOLD.to_vec(), 0),
            (bundled::ITALIC.to_vec(), 0),
            (bundled::BOLD_ITALIC.to_vec(), 0),
        )
        .expect("from_faces");
        assert_eq!(via_bytes.regular.data, via_faces.regular.data);
        assert_eq!(via_bytes.regular.offset, via_faces.regular.offset);
        assert_eq!(via_bytes.cell_metrics_px(12), via_faces.cell_metrics_px(12));
    }

    /// Asserts that a face whose bytes do not parse is reported as a
    /// font-parse error.
    ///
    /// Case: the configured bold family resolves to a file that is not a
    /// font, while every other face is valid.
    #[test]
    fn an_unparsable_face_is_reported_as_a_font_parse_error() {
        let result = TerminalFonts::from_bytes(
            bundled::REGULAR.to_vec(),
            b"not a font".to_vec(),
            bundled::ITALIC.to_vec(),
            bundled::BOLD_ITALIC.to_vec(),
        );
        assert!(matches!(result, Err(RendererError::FontParse)));
    }

    /// Asserts that `from_faces` sets the bundled UDEV Gothic 35 faces as
    /// the fallbacks, whatever the primary faces are.
    ///
    /// Case: the user configures a system family whose four faces all
    /// resolve to one file, and the grid still needs CJK coverage for every
    /// style.
    #[test]
    fn from_faces_sets_the_bundled_fallbacks() {
        let fonts = TerminalFonts::from_faces(
            (bundled::REGULAR.to_vec(), 0),
            (bundled::REGULAR.to_vec(), 0),
            (bundled::REGULAR.to_vec(), 0),
            (bundled::REGULAR.to_vec(), 0),
        )
        .expect("from_faces accepts JBM regular for all four faces");
        for (face, bytes) in [
            (FontFace::Regular, bundled::FALLBACK_REGULAR),
            (FontFace::Bold, bundled::FALLBACK_BOLD),
            (FontFace::Italic, bundled::FALLBACK_ITALIC),
            (FontFace::BoldItalic, bundled::FALLBACK_BOLD_ITALIC),
        ] {
            assert!(
                fonts.fallback_choice(&face).data == bytes,
                "{face:?} fallback is not the bundled face"
            );
        }
    }

    /// Asserts that `cell_metrics_px(12)` measures the cell of the bundled
    /// JetBrains Mono Nerd Font Mono Regular, with the underline below the
    /// baseline.
    ///
    /// Case: the terminal lays out its grid with the bundled font at the
    /// default 12 px size.
    #[test]
    fn jetbrains_mono_12px_metrics_are_sensible() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);
        assert_eq!(m.cell_size, Vec2::new(7.0, 15.0));
        assert_eq!(*m.baseline, 12.0);
        assert!(
            m.underline.position < 0.0,
            "underline.position = {} should be below baseline (negative)",
            m.underline.position
        );
    }

    /// Asserts that the bundled font at 12 px reports a non-zero
    /// `max_overflow`.
    ///
    /// Case: the terminal lays out the bundled font at 12 px, where a
    /// glyph like `W` rasterizes past the floored advance.
    #[test]
    fn cell_metrics_px_reports_nonzero_max_overflow() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);
        assert!(
            m.max_overflow > 0.0,
            "max_overflow = {} (expected > 0 driven by wide ASCII glyphs)",
            m.max_overflow
        );
    }

    /// Asserts that `max_overflow` is at least the overflow each of the
    /// four primary faces reaches on its own.
    ///
    /// Case: a program prints bold and italic text, whose glyphs can
    /// reach further past the cell than the regular face's.
    #[test]
    fn cell_metrics_px_max_overflow_covers_all_faces() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);
        let mut context = ScaleContext::new();

        for (name, face) in [
            ("Regular", &fonts.regular),
            ("Italic", &fonts.italic),
            ("Bold", &fonts.bold),
            ("BoldItalic", &fonts.bold_italic),
        ] {
            let face_overflow = max_ascii_overflow(&mut context, **face, 12.0, m.cell_size.x);
            assert!(
                face_overflow <= m.max_overflow,
                "{name} face overflow = {face_overflow} exceeds reported max_overflow = {}",
                m.max_overflow,
            );
        }
    }

    /// Asserts that the 24 px cell is double the 12 px one, give or take
    /// the pixel that flooring each axis can drop.
    ///
    /// Case: the user doubles the font size from 12 px to 24 px.
    #[test]
    fn metrics_scale_linearly_with_size() {
        let fonts = TerminalFonts::default();
        let m12 = fonts.cell_metrics_px(12);
        let m24 = fonts.cell_metrics_px(24);
        let excess = m24.cell_size - m12.cell_size * 2.0;
        assert!(
            excess.min_element() >= 0.0 && excess.max_element() <= 1.0,
            "24 px cell {} is not double the 12 px cell {}",
            m24.cell_size,
            m12.cell_size,
        );
    }

    /// Asserts that the cell never shrinks below one pixel on either axis.
    ///
    /// Case: a 1 px font measures a sub-pixel advance and line height.
    #[test]
    fn cell_metrics_px_cell_is_at_least_one_pixel() {
        assert_eq!(
            TerminalFonts::default().cell_metrics_px(1).cell_size,
            Vec2::ONE
        );
    }

    /// Asserts that each `FontFace` gets its own fallback face rather than
    /// one face wired to every style.
    ///
    /// Case: a program prints bold and italic Japanese text, which needs
    /// the bold and italic UDEV Gothic 35 faces.
    #[test]
    fn fallback_choice_returns_face_aware() {
        let fonts = TerminalFonts::default();
        let r_ptr = fonts.fallback_choice(&FontFace::Regular).data.as_ptr();
        let b_ptr = fonts.fallback_choice(&FontFace::Bold).data.as_ptr();
        let i_ptr = fonts.fallback_choice(&FontFace::Italic).data.as_ptr();
        let bi_ptr = fonts.fallback_choice(&FontFace::BoldItalic).data.as_ptr();
        assert_ne!(r_ptr, b_ptr, "Regular and Bold fallback share bytes");
        assert_ne!(r_ptr, i_ptr, "Regular and Italic fallback share bytes");
        assert_ne!(r_ptr, bi_ptr, "Regular and BoldItalic fallback share bytes");
        assert_ne!(b_ptr, i_ptr, "Bold and Italic fallback share bytes");
    }
}
