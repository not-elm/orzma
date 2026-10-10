//! Turning a glyph key into outlines: the face fallback chain, symbol
//! fitting, and mark placement.

use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Scaler, Source};
use swash::zeno::{Format, Vector};
use swash::{FontRef, GlyphId};

use crate::font::{FontFace, LoadedFont, TerminalFonts};
use crate::glyph::GlyphKey;

/// Which face in the fallback chain a glyph resolved through.
#[derive(Clone, Copy)]
pub enum GlyphTier {
    Primary,
    Fallback,
    Symbol,
}

/// Resolves a glyph for the requested codepoint, walking the fallback chain:
/// primary face → CJK fallback (`fallback_choice`) → symbol fallback
/// (`symbol`), trying the next only when the current's `glyph_id` is 0
/// (notdef).
///
/// Returns `(font, glyph_id, tier)` for the resolved face, or `None` when no
/// face in the chain contains the glyph.
///
/// A face whose glyph outlines to zero extent still resolves there. PUA
/// Nerd Font icons (U+E000–U+F8FF) resolve non-zero on the primary, so
/// they never reach the fallbacks.
pub fn resolve_glyph<'a>(
    fonts: &'a TerminalFonts,
    face: &FontFace,
    ch: char,
) -> Option<(&'a LoadedFont, GlyphId, GlyphTier)> {
    let primary = fonts.choice(face);
    let id = primary.glyph_id(ch);
    if id != 0 {
        return Some((primary, id, GlyphTier::Primary));
    }
    let fallback = fonts.fallback_choice(face);
    let id = fallback.glyph_id(ch);
    if id != 0 {
        return Some((fallback, id, GlyphTier::Fallback));
    }
    let symbol = &fonts.symbol;
    let id = symbol.glyph_id(ch);
    if id != 0 {
        return Some((symbol, id, GlyphTier::Symbol));
    }
    None
}

/// Shrinks a symbol-tier glyph so its rasterized width fits the monospace cell
/// advance, returning the original outline when it already fits or when
/// re-outlining at the reduced scale fails.
pub fn fit_symbol_to_cell(
    context: &mut ScaleContext,
    font: FontRef<'_>,
    glyph_id: GlyphId,
    px: f32,
    image: Image,
    cell_advance_px: f32,
) -> Image {
    let w = image.placement.width as f32;
    if cell_advance_px <= 0.0 || w <= cell_advance_px {
        return image;
    }
    let mut scaler = context
        .builder(font)
        .size(px * (cell_advance_px / w))
        .hint(false)
        .build();
    Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .render(&mut scaler, glyph_id)
        .unwrap_or(image)
}

/// Renders the key's marks at `px` px per em, each laid over `base`: a mark
/// whose outline reaches left of its origin sits at the pen position after
/// `base_id`'s advance, and any other mark is right-aligned to `base`'s
/// ink. A mark the font lacks, that renders to nothing, or whose box does
/// not overlap `base`'s box horizontally is left out.
// TODO: Place marks from GPOS anchor data. The advance and right-align
// rules put a Latin mark right of center on a fullwidth base and drop a
// mark that lands beside narrow CJK punctuation.
pub fn outline_marks(
    scaler: &mut Scaler,
    font: FontRef<'_>,
    px: f32,
    base: &Image,
    base_id: GlyphId,
    key: GlyphKey,
) -> Vec<Image> {
    let base_left = base.placement.left as f32;
    let base_right = base_left + base.placement.width as f32;
    let advance = font.glyph_metrics(&[]).scale(px).advance_width(base_id);
    let charmap = font.charmap();
    key.marks()
        .filter_map(|mark| {
            let id = charmap.map(mark);
            if id == 0 {
                return None;
            }
            let at_origin = scaler.scale_outline(id)?.bounds();
            let x = if at_origin.min.x < 0.0 {
                advance
            } else {
                base_right - at_origin.max.x.ceil()
            };
            let placed = render_outline(scaler, id, x)?;
            let placement = placed.placement;
            let left = placement.left as f32;
            let right = left + placement.width as f32;
            let inked = placement.width > 0 && placement.height > 0;
            (inked && left < base_right && right > base_left).then_some(placed)
        })
        .collect()
}

/// Renders `glyph_id`'s outline as an alpha mask with its origin `x` px
/// right of the pen position on the baseline.
fn render_outline(scaler: &mut Scaler, glyph_id: GlyphId, x: f32) -> Option<Image> {
    Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .offset(Vector::new(x, 0.0))
        .render(scaler, glyph_id)
}
