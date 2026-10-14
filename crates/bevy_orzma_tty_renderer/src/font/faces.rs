//! The faces the terminal grid draws with, and the metrics measured from
//! them.

use crate::bundled::{
    BOLD, BOLD_ITALIC, FALLBACK_BOLD, FALLBACK_BOLD_ITALIC, FALLBACK_ITALIC, FALLBACK_REGULAR,
    ITALIC, REGULAR, SYMBOL_REGULAR,
};
use crate::error::{RendererError, RendererResult};
use crate::font::metrics::CellGeometry;
use crate::font::{CellMetrics, FontFace, Underline};
use bevy::prelude::{Deref, Resource};
use swash::scale::ScaleContext;
use swash::{FontRef, GlyphId};

/// The faces the terminal grid draws with: four primary faces, the bundled
/// CJK fallback for each, the bundled symbol face, and the color emoji face
/// when one is loaded.
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
    /// The color emoji face, tried for a glyph the VT reserved two columns
    /// for when neither the primary nor the CJK fallback face carries it,
    /// before the symbol face; `None` when no emoji face is loaded. A glyph
    /// is taken from it only when the face draws the glyph in color.
    pub emoji: Option<LoadedFont>,
}

impl TerminalFonts {
    /// Constructs a `TerminalFonts` from four primary `(bytes, .ttc index)`
    /// pairs, loading each primary face at its own collection index.
    ///
    /// The fallback faces are always the bundled UDEV Gothic 35 faces and
    /// the symbol face is always the bundled Noto Sans Symbols 2, so callers
    /// supply neither. No emoji face is loaded.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::FontParse`] when a face's bytes are not a
    /// font, hold no face at its `.ttc` index, or hold a face that reports
    /// zero units per em. The error does not say which face failed.
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
            emoji: None,
        })
    }

    /// Constructs a `TerminalFonts` from four owned TTF byte buffers, one
    /// per primary face, loading every primary face at collection index 0.
    ///
    /// The fallback faces are always the bundled UDEV Gothic 35 faces and
    /// the symbol face is always the bundled Noto Sans Symbols 2, so callers
    /// supply neither. No emoji face is loaded.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::FontParse`] when a face's bytes are not a
    /// font or hold a face that reports zero units per em. The error does
    /// not say which face failed.
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

    /// Loads the color emoji face at `.ttc` index `index` of `bytes` into
    /// [`TerminalFonts::emoji`].
    ///
    /// On failure the emoji face is left as it was.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::FontParse`] when `bytes` are not a font, hold
    /// no face at `index`, or hold a face that reports zero units per em.
    pub fn set_emoji(&mut self, bytes: &'static [u8], index: u32) -> RendererResult {
        self.emoji = Some(LoadedFont::new(bytes, index as usize)?);
        Ok(())
    }

    /// Returns full pixel metrics for the regular face at the requested
    /// physical pixel size.
    pub fn cell_metrics_px(&self, phys_size_px: u16) -> CellMetrics {
        let font = self.regular.0;
        let px = f32::from(phys_size_px);
        let metrics = font.metrics(&[]);
        let zero_advance = font
            .glyph_metrics(&[])
            .advance_width(font.charmap().map('0'));
        let geometry = CellGeometry::measure(&metrics, zero_advance, px);
        let cell_size = geometry.cell_size();
        let mut context = ScaleContext::new();
        let max_overflow = [&self.regular, &self.italic, &self.bold, &self.bold_italic]
            .into_iter()
            .map(|font| max_ascii_overflow(&mut context, font.0, px, cell_size.x))
            .fold(0.0_f32, f32::max);

        CellMetrics {
            cell_size,
            baseline: geometry.baseline(),
            underline: Underline::measure(font, &metrics, px),
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
            emoji: None,
        }
    }
}

#[derive(Clone, Deref)]
pub struct LoadedFont(FontRef<'static>);

impl LoadedFont {
    /// Loads the face at `.ttc` index `index` of `bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::FontParse`] when `bytes` are not a font, hold
    /// no face at `index`, or hold a face whose `head` table reports zero
    /// units per em.
    pub fn new(bytes: &'static [u8], index: usize) -> RendererResult<Self> {
        let font = FontRef::from_index(bytes, index)
            .filter(|font| font.metrics(&[]).units_per_em != 0)
            .ok_or(RendererError::FontParse)?;
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
    use bevy::prelude::Vec2;

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

    /// Asserts that a face whose `head` table reports zero units per em is
    /// reported as a font-parse error.
    ///
    /// Case: the configured regular family resolves to a damaged font file
    /// whose `head` table lost its units-per-em value.
    #[test]
    fn a_face_with_zero_units_per_em_is_reported_as_a_font_parse_error() {
        let result = TerminalFonts::from_bytes(
            with_zero_units_per_em(bundled::REGULAR),
            bundled::BOLD.to_vec(),
            bundled::ITALIC.to_vec(),
            bundled::BOLD_ITALIC.to_vec(),
        );
        assert!(matches!(result, Err(RendererError::FontParse)));
    }

    fn with_zero_units_per_em(font: &[u8]) -> Vec<u8> {
        let mut bytes = font.to_vec();
        let table_count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        let head = (0..table_count)
            .map(|index| 12 + 16 * index)
            .find(|record| &bytes[*record..*record + 4] == b"head")
            .map(|record| {
                let offset = bytes[record + 8..record + 12]
                    .try_into()
                    .expect("a table record holds a four-byte offset");
                u32::from_be_bytes(offset) as usize
            })
            .expect("the font has a head table");
        bytes[head + 18..head + 20].copy_from_slice(&[0, 0]);
        bytes
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

    /// Asserts that the default fonts and fonts built from faces carry no
    /// emoji face.
    ///
    /// Case: the app starts before the font bridge has searched the system
    /// for an emoji font.
    #[test]
    fn fonts_start_without_an_emoji_face() {
        assert!(TerminalFonts::default().emoji.is_none());
        let built = TerminalFonts::from_bytes(
            bundled::REGULAR.to_vec(),
            bundled::BOLD.to_vec(),
            bundled::ITALIC.to_vec(),
            bundled::BOLD_ITALIC.to_vec(),
        )
        .expect("from_bytes");
        assert!(built.emoji.is_none());
    }

    /// Asserts that `set_emoji` loads the face at the given index into the
    /// emoji slot.
    ///
    /// Case: the font bridge finds the system's emoji font at startup.
    #[test]
    fn set_emoji_loads_the_face_into_the_emoji_slot() {
        let mut fonts = TerminalFonts::default();

        fonts
            .set_emoji(bundled::SYMBOL_REGULAR, 0)
            .expect("Noto Sans Symbols 2 is a font");

        let emoji = fonts.emoji.as_ref().expect("the emoji slot is filled");
        assert_eq!(emoji.data, bundled::SYMBOL_REGULAR);
    }

    /// Asserts that `set_emoji` reports bytes that are not a font as a
    /// font-parse error and leaves the emoji slot empty.
    ///
    /// Case: the system's emoji font file is damaged.
    #[test]
    fn set_emoji_rejects_bytes_that_are_not_a_font() {
        let mut fonts = TerminalFonts::default();

        let result = fonts.set_emoji(b"not a font", 0);

        assert!(matches!(result, Err(RendererError::FontParse)));
        assert!(fonts.emoji.is_none());
    }
}
