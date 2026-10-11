//! Turning a glyph key into outlines: the face fallback chain, symbol
//! fitting, and mark placement.

use crate::font::{FontFace, LoadedFont, TerminalFonts};
use crate::glyph::GlyphKey;
use std::iter::once;
use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Scaler, Source};
use swash::shape::ShapeContext;
use swash::shape::cluster::Glyph;
use swash::text::Codepoint;
use swash::text::cluster::{CharCluster, Parser, Token};
use swash::zeno::{Format, Vector};
use swash::{FontRef, GlyphId};

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

/// Renders the key's marks at `px` px per em, each laid over `base`.
///
/// A mark the face attaches through its anchors, to the base or to the
/// mark before it, sits where the anchors put it. Of the other marks, one
/// whose outline reaches left of its origin sits at the pen position after
/// `base_id`'s advance, and the rest are right-aligned to `base`'s ink. The
/// key's characters are shaped as given, so the marks are placed over
/// `base_id` even where the face carries the base and a mark precomposed.
/// A mark the face lacks, that renders to nothing, or whose box does not
/// overlap `base`'s box horizontally is left out.
// TODO: Draw the base the face substitutes before a mark, such as the
// dotless i it puts under an accent above, in place of `base_id`.
pub fn outline_marks(
    shape_context: &mut ShapeContext,
    scaler: &mut Scaler,
    font: FontRef<'_>,
    px: f32,
    base: &Image,
    base_id: GlyphId,
    key: GlyphKey,
) -> Vec<Image> {
    if key.marks().next().is_none() {
        return Vec::new();
    }
    let Some(base_char) = char::from_u32(key.codepoint) else {
        return Vec::new();
    };
    let glyphs = shape_as_given(shape_context, font, px, base_char, key);
    let Some((shaped_base, marks)) = glyphs.split_first() else {
        return Vec::new();
    };
    let base_advance = font.glyph_metrics(&[]).scale(px).advance_width(base_id);
    let mut pen = shaped_base.advance;
    let mut placed = Vec::new();
    for mark in marks {
        let after_previous = pen;
        pen += mark.advance;
        if mark.id == 0 {
            continue;
        }
        let image = if mark.info.is_mark() {
            [after_previous, shaped_base.advance]
                .into_iter()
                .find_map(|origin| {
                    render_outline(scaler, mark.id, origin + mark.x, mark.y)
                        .filter(|image| is_inked_over(image, base))
                })
        } else {
            render_unanchored(scaler, mark.id, base, base_advance)
                .filter(|image| is_inked_over(image, base))
        };
        placed.extend(image);
    }
    placed
}

/// Shapes `base` followed by `key`'s marks with `font` at `px` px per em,
/// keeping the characters as given rather than swapping in a precomposed
/// form, and returns the glyphs in order.
fn shape_as_given(
    shape_context: &mut ShapeContext,
    font: FontRef<'_>,
    px: f32,
    base: char,
    key: GlyphKey,
) -> Vec<Glyph> {
    let script = base.script();
    let charmap = font.charmap();
    let text: String = once(base).chain(key.marks()).collect();
    let tokens = text.char_indices().map(|(offset, ch)| Token {
        ch,
        offset: offset as u32,
        len: ch.len_utf8() as u8,
        info: ch.properties().into(),
        data: 0,
    });
    let mut parser = Parser::new(script, tokens);
    let mut cluster = CharCluster::new();
    let mut shaper = shape_context.builder(font).script(script).size(px).build();
    while parser.next(&mut cluster) {
        cluster.map(|ch| {
            if text.contains(ch) {
                charmap.map(ch)
            } else {
                0
            }
        });
        shaper.add_cluster(&cluster);
    }
    let mut glyphs = Vec::new();
    shaper.shape_with(|cluster| glyphs.extend_from_slice(cluster.glyphs));
    glyphs
}

/// Renders a mark the face does not anchor: at the pen position after the
/// base's `base_advance` when its outline reaches left of its origin, and
/// right-aligned to `base`'s ink otherwise.
fn render_unanchored(
    scaler: &mut Scaler,
    mark_id: GlyphId,
    base: &Image,
    base_advance: f32,
) -> Option<Image> {
    let at_origin = scaler.scale_outline(mark_id)?.bounds();
    let x = if at_origin.min.x < 0.0 {
        base_advance
    } else {
        let base_right = base.placement.left as f32 + base.placement.width as f32;
        base_right - at_origin.max.x.ceil()
    };
    render_outline(scaler, mark_id, x, 0.0)
}

/// Renders `glyph_id`'s outline as an alpha mask with its origin `x` px
/// right of and `y` px above the pen position on the baseline.
fn render_outline(scaler: &mut Scaler, glyph_id: GlyphId, x: f32, y: f32) -> Option<Image> {
    Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .offset(Vector::new(x, y))
        .render(scaler, glyph_id)
}

/// Whether `mark` has ink and its box overlaps `base`'s box horizontally.
fn is_inked_over(mark: &Image, base: &Image) -> bool {
    let (mark, base) = (mark.placement, base.placement);
    let inked = mark.width > 0 && mark.height > 0;
    inked && mark.left < base.left + base.width as i32 && mark.left + mark.width as i32 > base.left
}
