//! Turning a glyph key into outlines: the face fallback chain, symbol
//! fitting, and mark placement.

use crate::font::{FontFace, TerminalFonts};
use crate::glyph::GlyphKey;
use ab_glyph::{Font, FontArc, GlyphId, OutlinedGlyph, PxScale, Rect, ScaleFont, point};

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
) -> Option<(&'a FontArc, GlyphId, GlyphTier)> {
    let primary = fonts.choice(face);
    let id = primary.glyph_id(ch);
    if id.0 != 0 {
        return Some((primary, id, GlyphTier::Primary));
    }
    let fallback = fonts.fallback_choice(face);
    let id = fallback.glyph_id(ch);
    if id.0 != 0 {
        return Some((fallback, id, GlyphTier::Fallback));
    }
    let symbol = &fonts.symbol;
    let id = symbol.glyph_id(ch);
    if id.0 != 0 {
        return Some((symbol, id, GlyphTier::Symbol));
    }
    None
}

/// Shrinks a symbol-tier glyph so its rasterized width fits the monospace cell
/// advance, returning the original outline when it already fits or when
/// re-outlining at the reduced scale fails.
pub fn fit_symbol_to_cell(
    font: &FontArc,
    glyph_id: GlyphId,
    scale_value: f32,
    outlined: OutlinedGlyph,
    cell_advance_px: f32,
) -> OutlinedGlyph {
    let w = outlined.px_bounds().width();
    if cell_advance_px <= 0.0 || w <= cell_advance_px {
        return outlined;
    }
    let fitted = PxScale::from(scale_value * (cell_advance_px / w));
    font.outline_glyph(glyph_id.with_scale(fitted))
        .unwrap_or(outlined)
}

/// The smallest rect containing both `a` and `b`.
pub fn union(a: Rect, b: Rect) -> Rect {
    Rect {
        min: point(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
        max: point(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
    }
}

/// Outlines the key's marks at `scale`, each laid over `base`: a mark
/// whose outline reaches left of its origin sits at the pen position
/// after `base_id`'s advance, and any other mark is right-aligned to
/// `base`'s ink. A mark the font lacks, that outlines to nothing, or
/// whose box does not overlap `base`'s box horizontally is left out.
// TODO: Place marks from GPOS anchor data. The advance and right-align
// rules put a Latin mark right of center on a fullwidth base and drop a
// mark that lands beside narrow CJK punctuation.
pub fn outline_marks(
    font: &FontArc,
    scale: PxScale,
    base: &OutlinedGlyph,
    base_id: GlyphId,
    key: GlyphKey,
) -> Vec<OutlinedGlyph> {
    let base_bounds = base.px_bounds();
    let advance = font.as_scaled(scale).h_advance(base_id);
    key.marks()
        .filter_map(|mark| {
            let id = font.glyph_id(mark);
            if id.0 == 0 {
                return None;
            }
            let at_origin = font.outline_glyph(id.with_scale(scale))?.px_bounds();
            let x = if at_origin.min.x < 0.0 {
                advance
            } else {
                base_bounds.max.x - at_origin.max.x
            };
            let placed = font.outline_glyph(id.with_scale_and_position(scale, point(x, 0.0)))?;
            let placed_bounds = placed.px_bounds();
            let overlaps =
                placed_bounds.min.x < base_bounds.max.x && placed_bounds.max.x > base_bounds.min.x;
            overlaps.then_some(placed)
        })
        .collect()
}
