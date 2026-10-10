//! The CPU-side glyph atlas: rasterized sprites packed onto shelves of one
//! coverage texture.

use crate::error::RendererResult;
use crate::font::TerminalFonts;
use crate::glyph::{
    GlyphKey,
    outline::{GlyphTier, fit_symbol_to_cell, outline_marks, resolve_glyph},
};
use bevy::{platform::collections::HashMap, prelude::*};
use std::iter::once;
use swash::scale::outline::Outline;
use swash::scale::{Render, ScaleContext, Scaler, Source};
use swash::zeno::{Bounds, Format};
use swash::{FontRef, GlyphId};

pub struct TerminalGlyphAtlasPlugin;

impl Plugin for TerminalGlyphAtlasPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GlyphAtlas>();
    }
}

/// Position and size of a rasterized glyph inside the atlas, plus the
/// rasterizer's reported origin offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphRect {
    /// Left column of the glyph in atlas pixels.
    pub u: u16,
    /// Top row of the glyph in atlas pixels.
    pub v: u16,
    /// Width of the rasterized bitmap in pixels.
    pub w: u16,
    /// Height of the rasterized bitmap in pixels.
    pub h: u16,
    /// Horizontal bearing from the glyph origin (may be negative).
    pub offset_x: i16,
    /// Vertical bearing from the glyph origin (may be negative).
    pub offset_y: i16,
}

/// CPU-side R8Unorm atlas for rasterized glyphs.
///
/// When the atlas is full, it clears every packed glyph and restarts from
/// the top-left.
#[derive(Resource)]
pub struct GlyphAtlas {
    /// One byte of alpha coverage per pixel, row-major.
    pub pixels: Vec<u8>,
    /// All glyphs currently packed into the atlas.
    pub glyphs: HashMap<GlyphKey, GlyphRect>,
    /// Bumped on every rasterization, including one that clears a full
    /// atlas first. The GPU texture picks up `pixels` only when this
    /// value changes.
    pub generation: u64,
    /// Bumped each time a full atlas is cleared to make room for a glyph.
    pub restarts: u64,
    shelves: Shelves,
    context: ScaleContext,
}

impl GlyphAtlas {
    /// Creates an empty atlas with the given pixel dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            pixels: vec![0; (width * height) as usize],
            glyphs: HashMap::new(),
            generation: 0,
            restarts: 0,
            shelves: Shelves::new(width, height),
            context: ScaleContext::new(),
        }
    }

    #[inline]
    pub const fn width(&self) -> u32 {
        self.shelves.width
    }

    #[inline]
    pub const fn height(&self) -> u32 {
        self.shelves.height
    }

    /// Returns the rect for the keyed glyph, rasterizing and packing it on
    /// first use.
    ///
    /// The key's marks are composed onto the glyph at the same face and
    /// scale; a mark the face lacks, that outlines to nothing, or that
    /// would not overlap the glyph is left out, and a glyph resolved
    /// through the symbol face takes no marks.
    ///
    /// Returns `None` when the codepoint is not a valid Unicode scalar, no
    /// face in the fallback chain carries it, the glyph has zero extent
    /// (e.g. ASCII space), or the sprite is larger than the atlas.
    pub fn get_or_insert(&mut self, key: GlyphKey, fonts: &TerminalFonts) -> Option<GlyphRect> {
        if let Some(r) = self.glyphs.get(&key) {
            return Some(*r);
        }
        let ch = char::from_u32(key.codepoint)?;
        let (font, glyph_id, tier) = resolve_glyph(fonts, &key.face, ch)?;
        // let outlined = font.outline_glyph(glyph_id.with_scale(scale))?;
        // let outlined = if matches!(tier, GlyphTier::Symbol) {
        //     fit_symbol_to_cell(
        //         font,
        //         glyph_id,
        //         scale_value,
        //         outlined,
        //         fonts.cell_advance_px(key.size_px),
        //     )
        // } else {
        //     outlined
        // };
        // let marks = if matches!(tier, GlyphTier::Symbol) {
        //     Vec::new()
        // } else {
        //     outline_marks(font, scale, &outlined, glyph_id, key) };
        // let bounds = marks
        //     .iter()
        //     .map(OutlinedGlyph::px_bounds)
        //     .fold(outlined.px_bounds(), union);
        // TODO: 単純にNoneを返すだけでいいのか検討する
        let bounds = self.outline_bounds(font.font_ref(), glyph_id)?;
        let w = bounds.width().ceil() as u16;
        let h = bounds.height().ceil() as u16;
        if w == 0 || h == 0 || u32::from(w) > self.width() || u32::from(h) > self.height() {
            return None;
        }

        self.write_glyph_pixels(
            font.font_ref(),
            key.size_px as f32,
            glyph_id,
            &bounds,
            &[bounds],
        );
        self.shelves.new_line_if_need(w);
        if self.shelves.would_overflow(h) {
            self.shelves.clear();
            self.pixels.fill(0);
            self.glyphs.clear();
            self.restarts = self.restarts.wrapping_add(1);
        }
        let u = self.shelves.shelf.x as u16;
        let v = self.shelves.y as u16;
        self.shelves.advance_x(w);
        self.shelves.adjust_shelf_height(h);
        self.generation = self.generation.wrapping_add(1);
        let rect = GlyphRect {
            u,
            v,
            w,
            h,
            offset_x: bounds.min.x.floor() as i16,
            offset_y: bounds.min.y.floor() as i16,
        };
        self.glyphs.insert(key, rect);
        Some(rect)
    }

    fn build_scaler<'a>(&'a mut self, font: FontRef<'a>, font_size: f32) -> Scaler<'a> {
        self.context
            .builder(font)
            .size(font_size)
            //TODO: Should set true?
            .hint(false)
            .build()
    }

    fn outline_bounds<'a>(&'a mut self, font: FontRef<'a>, glyph_id: GlyphId) -> Option<Bounds> {
        let outline = self.context.builder(font).build().scale_outline(glyph_id)?;
        // TODO: markの処理
        Some(outline.bounds())
    }

    fn write_glyph_pixels(
        &mut self,
        font: FontRef<'_>,
        font_size: f32,
        glyph_id: GlyphId,
        union_bounds: &Bounds,
        bounds_parts: &[Bounds],
    ) {
        let mut scaler = self.build_scaler(font, font_size);
        let Some(image) = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, glyph_id)
        else {
            return;
        };

        for local in bounds_parts {
            let dx = (local.min.x - union_bounds.min.x) as u32;
            let dy = (local.min.y - union_bounds.min.y) as u32;
            self.write_image_pixels(&image, dx, dy);
        }
    }

    /// Blends `image`'s coverage into the current shelf position,
    /// shifted right by `dx` and down by `dy` pixels, keeping the higher coverage where pixels overlap.
    fn write_image_pixels(&mut self, image: &swash::scale::image::Image, dx: u32, dy: u32) {
        let width = image.placement.width as usize;
        if width == 0 {
            return;
        }
        let atlas_width = self.shelves.width as usize;
        let u = (self.shelves.shelf.x + dx) as usize;
        let v = (self.shelves.y + dy) as usize;
        for (row, coverage) in image.data.chunks_exact(width).enumerate() {
            let start = (v + row) * atlas_width + u;
            let Some(slots) = self.pixels.get_mut(start..start + width) else {
                continue;
            };
            for (slot, alpha) in slots.iter_mut().zip(coverage) {
                *slot = (*slot).max(*alpha);
            }
        }
    }
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new(1024, 1024)
    }
}

struct Shelves {
    pub width: u32,
    pub height: u32,
    pub y: u32,
    shelf: Shelf,
}

impl Shelves {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            y: 0,
            shelf: Shelf::default(),
        }
    }

    #[inline]
    pub fn new_line_if_need(&mut self, font_width: u16) {
        if self.width < self.shelf.x + font_width as u32 {
            self.shelf.x = 0;
            self.y = self.y.saturating_add(self.shelf.height);
            self.shelf.height = 0;
        }
    }

    /// Whether the current shelf, grown to at least `height` pixels,
    /// would not fit below the current row.
    #[inline]
    pub fn would_overflow(&self, height: u16) -> bool {
        self.height < self.y + self.shelf.height.max(u32::from(height))
    }

    #[inline]
    pub fn clear(&mut self) {
        self.shelf.x = 0;
        self.y = 0;
        self.shelf.height = 0;
    }

    #[inline]
    pub fn advance_x(&mut self, font_width: u16) {
        self.shelf.x = self.shelf.x.saturating_add(font_width as u32);
    }

    #[inline]
    pub fn adjust_shelf_height(&mut self, font_height: u16) {
        self.shelf.height = self.shelf.height.max(font_height as u32);
    }
}

#[derive(Default)]
struct Shelf {
    pub height: u32,
    pub x: u32,
}

/// The pixel box `image` covers, relative to the pen origin on the baseline with +y down.
fn sprite_rect(image: &swash::scale::image::Image) -> IRect {
    let placement = image.placement;
    let min = IVec2::new(placement.left, -placement.top);
    let size = IVec2::new(placement.width as i32, placement.height as i32);
    IRect::from_corners(min, min + size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{FontFace, LoadedFont};
    use swash::scale::image::Image;
    use swash::scale::{Render, ScaleContext, Source};
    use swash::zeno::Format;

    #[test]
    fn returned_rect_matches_written_pixels() {
        let mut atlas = GlyphAtlas::new(256, 256);
        let fonts = TerminalFonts::default();
        let key = GlyphKey::new(FontFace::Regular, 'A' as u32, 24);

        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("ASCII glyph should rasterize");
        assert_eq!(rect.u, 0, "first glyph must start at the left edge");
        assert_eq!(rect.v, 0, "first glyph must start at the top edge");

        let has_ink = sprite_pixels(&atlas, rect).iter().any(|alpha| *alpha > 0);
        assert!(has_ink, "returned rect must cover rasterized pixels");

        let rect2 = atlas
            .get_or_insert(key, &fonts)
            .expect("cached glyph lookup should succeed");
        assert_eq!(rect, rect2);
    }

    #[test]
    fn latin_renders_through_primary() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from('a'), 24);
        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("'a' must rasterize");
        assert!(rect.w > 0 && rect.h > 0, "'a' rect must be non-empty");
    }

    #[test]
    fn cjk_renders_through_fallback() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        // 'あ' (HIRAGANA LETTER A, U+3042) — present in UDEVGothic35,
        // absent from JetBrains Mono. Before this change, returned None.
        let key = GlyphKey::new(FontFace::Regular, 0x3042, 24);
        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("'あ' must rasterize via UDEVGothic35 fallback");
        assert!(rect.w > 0 && rect.h > 0, "'あ' rect must be non-empty");
    }

    #[test]
    fn nerd_font_pua_stays_on_primary() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        // U+E0B0 (Powerline right-pointing arrow) — present in JBM Nerd
        // Font Mono's PUA. The primary path must resolve it; UDEVGothic35
        // doesn't carry Nerd Font glyphs, so a fallback-only resolution
        // would either fail or return a different glyph.
        let key = GlyphKey::new(FontFace::Regular, 0xE0B0, 24);
        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("Powerline glyph U+E0B0 must rasterize via primary");
        assert!(rect.w > 0 && rect.h > 0, "U+E0B0 rect must be non-empty");
    }

    #[test]
    fn unknown_codepoint_returns_none() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        // U+1FFFFE — Plane 1 unassigned, not in either font.
        let key = GlyphKey::new(FontFace::Regular, 0x1FFFFE, 24);
        let result = atlas.get_or_insert(key, &fonts);
        assert!(
            result.is_none(),
            "unknown codepoint must return None (tofu suppression)"
        );
    }

    #[test]
    fn checkbox_marks_render_through_symbol_fallback() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let size = 24u16;
        let cell_w = fonts.cell_advance_px(size);
        // ☐ ☑ ☒ ✔ — Miscellaneous Symbols / Dingbats marks that interactive
        // TUIs (e.g. Claude Code's multi-select) draw for checkbox state.
        // Absent from BOTH JetBrains Mono Nerd Font and UDEVGothic35, so
        // before the symbol fallback they returned None and rendered blank.
        for codepoint in [0x2610u32, 0x2611, 0x2612, 0x2714] {
            let key = GlyphKey::new(FontFace::Regular, codepoint, size);
            let rect = atlas
                .get_or_insert(key, &fonts)
                .unwrap_or_else(|| panic!("U+{codepoint:04X} must rasterize via symbol fallback"));
            assert!(
                rect.w > 0 && rect.h > 0,
                "U+{codepoint:04X} rect must be non-empty"
            );
            // The proportional symbol glyph is shrunk to the monospace cell so
            // it does not overflow and overdraw the neighbouring cell.
            assert!(
                rect.w <= cell_w.ceil() as u16 + 1,
                "U+{codepoint:04X} width {} must fit cell advance {cell_w:.1}",
                rect.w
            );
        }
    }

    /// The atlas pixels inside `rect`, row-major.
    fn sprite_pixels(atlas: &GlyphAtlas, rect: GlyphRect) -> Vec<u8> {
        let width = atlas.width() as usize;
        (0..usize::from(rect.h))
            .flat_map(|dy| {
                let row = (usize::from(rect.v) + dy) * width + usize::from(rect.u);
                atlas.pixels[row..row + usize::from(rect.w)].iter().copied()
            })
            .collect()
    }

    /// Renders `ch` from `font` on its own at `size_px` px per em, unhinted,
    /// as an alpha mask.
    fn standalone_raster(font: &LoadedFont, ch: char, size_px: u16) -> Image {
        let mut context = ScaleContext::new();
        let mut scaler = context
            .builder(**font)
            .size(f32::from(size_px))
            .hint(false)
            .build();
        Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, font.glyph_id(ch))
            .expect("the glyph renders on its own")
    }

    /// Asserts that `rect` has `image`'s size and holds its coverage pixel for
    /// pixel.
    fn assert_sprite_is(atlas: &GlyphAtlas, rect: GlyphRect, image: &Image) {
        let placement = image.placement;
        assert_eq!(
            (u32::from(rect.w), u32::from(rect.h)),
            (placement.width, placement.height)
        );
        assert_eq!(sprite_pixels(atlas, rect), image.data);
    }

    /// Asserts that `rect` is the same sprite as `base`: size, bearing, and
    /// pixels.
    fn assert_same_sprite(atlas: &GlyphAtlas, rect: GlyphRect, base: GlyphRect) {
        assert_eq!(
            (rect.w, rect.h, rect.offset_x, rect.offset_y),
            (base.w, base.h, base.offset_x, base.offset_y)
        );
        assert_eq!(sprite_pixels(atlas, rect), sprite_pixels(atlas, base));
    }

    /// The `(w, h)` of `key`'s sprite, measured in a fresh default atlas.
    fn sprite_size(key: GlyphKey, fonts: &TerminalFonts) -> (u32, u32) {
        let rect = GlyphAtlas::default()
            .get_or_insert(key, fonts)
            .expect("the glyph rasterizes");
        (u32::from(rect.w), u32::from(rect.h))
    }

    /// Asserts that a glyph composed with a mark above it is taller than
    /// the bare glyph, starts higher, keeps the bare glyph's left edge,
    /// and gets an atlas entry of its own.
    ///
    /// Case: a shell echoes `e` followed by a combining acute accent.
    #[test]
    fn a_composed_glyph_grows_upward_and_gets_its_own_entry() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let base = GlyphKey::new(FontFace::Regular, u32::from('e'), 24);
        let accented = base.with_marks(['\u{0301}']);
        let base_rect = atlas.get_or_insert(base, &fonts).expect("'e' rasterizes");
        let accented_rect = atlas
            .get_or_insert(accented, &fonts)
            .expect("'e' with an accent rasterizes");
        assert!(
            accented_rect.h > base_rect.h,
            "{accented_rect:?} vs {base_rect:?}"
        );
        assert!(accented_rect.offset_y < base_rect.offset_y);
        assert_eq!(accented_rect.offset_x, base_rect.offset_x);
        assert_eq!(atlas.glyphs.len(), 2);
    }

    /// Asserts that a composed sprite carries ink in the rows above the
    /// bare glyph's top.
    ///
    /// Case: a user reads `é` on screen and expects to see the accent.
    #[test]
    fn a_composed_sprite_has_ink_above_the_base() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let base = GlyphKey::new(FontFace::Regular, u32::from('e'), 24);
        let base_rect = atlas.get_or_insert(base, &fonts).expect("'e' rasterizes");
        let rect = atlas
            .get_or_insert(base.with_marks(['\u{0301}']), &fonts)
            .expect("'e' with an accent rasterizes");
        assert!(rect.offset_y < base_rect.offset_y);
        let rows_above_base = usize::from(base_rect.offset_y.abs_diff(rect.offset_y));
        let pixels = sprite_pixels(&atlas, rect);
        let has_ink_above = pixels[..rows_above_base * usize::from(rect.w)]
            .iter()
            .any(|alpha| *alpha > 0);
        assert!(has_ink_above, "the accent leaves ink above the letter");
    }

    /// Asserts that a spacing voiced-sound mark is laid over the kana's
    /// top-right rather than beside it, so the sprite keeps the kana's
    /// width and left edge.
    ///
    /// Case: a shell echoes `か` followed by a combining voiced sound
    /// mark through the CJK fallback face.
    #[test]
    fn a_spacing_mark_is_laid_over_the_base() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let base = GlyphKey::new(FontFace::Regular, u32::from('か'), 24);
        let base_rect = atlas.get_or_insert(base, &fonts).expect("か rasterizes");
        let rect = atlas
            .get_or_insert(base.with_marks(['\u{3099}']), &fonts)
            .expect("か with a voiced sound mark rasterizes");
        assert_eq!(rect.w, base_rect.w, "{rect:?} vs {base_rect:?}");
        assert_eq!(rect.offset_x, base_rect.offset_x);
        assert!(rect.offset_y <= base_rect.offset_y);
        let base_pixels = sprite_pixels(&atlas, base_rect);
        let pixels = sprite_pixels(&atlas, rect);
        assert_ne!(pixels, base_pixels, "the mark leaves ink");
    }

    /// Asserts that a glyph whose only mark is missing from its face
    /// rasterizes pixel for pixel as the bare glyph does.
    ///
    /// Case: a program prints a letter followed by a mark the terminal's
    /// font does not carry.
    #[test]
    fn a_mark_missing_from_the_face_is_dropped() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let base = GlyphKey::new(FontFace::Regular, u32::from('e'), 24);
        let base_rect = atlas.get_or_insert(base, &fonts).expect("'e' rasterizes");
        let marked_rect = atlas
            .get_or_insert(base.with_marks(['\u{1FFFE}']), &fonts)
            .expect("'e' with an unknown mark rasterizes");
        assert_eq!((marked_rect.w, marked_rect.h), (base_rect.w, base_rect.h));
        assert_eq!(
            sprite_pixels(&atlas, marked_rect),
            sprite_pixels(&atlas, base_rect)
        );
    }

    /// Asserts that a glyph taller than the space left below the current
    /// shelf restarts the atlas instead of being clipped.
    ///
    /// Case: a tall bare glyph arrives when the atlas is nearly full.
    #[test]
    fn a_glyph_taller_than_the_remaining_space_restarts_the_atlas() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::new(20, 24);
        let first = atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('A'), 24), &fonts)
            .expect("'A' rasterizes");
        assert_eq!(first.v, 0);
        let generation = atlas.generation;
        let tall = atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('j'), 24), &fonts)
            .expect("'j' rasterizes");
        assert_eq!(tall.v, 0, "{tall:?}");
        assert!(u32::from(tall.v) + u32::from(tall.h) <= atlas.height());
        assert_eq!(atlas.glyphs.len(), 1);
        assert!(atlas.generation > generation);
    }

    /// Asserts that a glyph resolved through the symbol face takes no
    /// marks and rasterizes pixel for pixel as the bare glyph does.
    ///
    /// Case: a TUI draws a checked checkbox followed by a combining
    /// enclosing keycap.
    #[test]
    fn a_symbol_tier_glyph_takes_no_marks() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let base = GlyphKey::new(FontFace::Regular, 0x2611, 24);
        let base_rect = atlas.get_or_insert(base, &fonts).expect("☑ rasterizes");
        let marked_rect = atlas
            .get_or_insert(base.with_marks(['\u{20E3}']), &fonts)
            .expect("☑ with a keycap mark rasterizes");
        assert_eq!((marked_rect.w, marked_rect.h), (base_rect.w, base_rect.h));
        assert_eq!(
            (marked_rect.offset_x, marked_rect.offset_y),
            (base_rect.offset_x, base_rect.offset_y)
        );
        assert_eq!(
            sprite_pixels(&atlas, marked_rect),
            sprite_pixels(&atlas, base_rect)
        );
    }

    /// Asserts that a mark whose placed box does not overlap the base is
    /// dropped, leaving the bare glyph's sprite.
    ///
    /// Case: a program prints an ideographic comma followed by a
    /// combining grave accent.
    #[test]
    fn a_mark_beside_the_base_is_dropped() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let base = GlyphKey::new(FontFace::Regular, u32::from('、'), 24);
        let base_rect = atlas.get_or_insert(base, &fonts).expect("、 rasterizes");
        let marked_rect = atlas
            .get_or_insert(base.with_marks(['\u{0300}']), &fonts)
            .expect("、 with a grave accent rasterizes");
        assert_eq!((marked_rect.w, marked_rect.h), (base_rect.w, base_rect.h));
        assert_eq!(
            sprite_pixels(&atlas, marked_rect),
            sprite_pixels(&atlas, base_rect)
        );
    }

    /// Asserts that a sprite larger than the atlas is refused rather than
    /// written clipped.
    ///
    /// Case: a glyph is requested at a size the configured atlas cannot
    /// hold.
    #[test]
    fn a_sprite_larger_than_the_atlas_is_refused() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::new(4, 4);
        let key = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        assert!(atlas.get_or_insert(key, &fonts).is_none());
        assert!(atlas.glyphs.is_empty());
    }

    /// Asserts that a bare glyph's first lookup packs its primary-face raster
    /// as is, with the raster's size, left bearing, and coverage, and records
    /// the rect under its key.
    ///
    /// Case: a shell prints an ASCII letter for the first time in a session.
    #[test]
    fn a_bare_glyph_is_packed_as_its_primary_face_raster() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let primary = standalone_raster(fonts.choice(&FontFace::Regular), 'A', 24);
        let generation = atlas.generation;

        let rect = atlas.get_or_insert(key, &fonts).expect("'A' rasterizes");

        assert_sprite_is(&atlas, rect, &primary);
        assert_eq!(i32::from(rect.offset_x), primary.placement.left);
        assert_eq!(atlas.glyphs.get(&key), Some(&rect));
        assert_ne!(atlas.generation, generation);
    }

    /// Asserts that a second lookup of the same key returns the first rect
    /// without rasterizing again.
    ///
    /// Case: the renderer draws the same letter on every frame.
    #[test]
    fn a_repeated_key_returns_the_cached_rect_without_rasterizing() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let first = atlas.get_or_insert(key, &fonts).expect("'A' rasterizes");
        let generation = atlas.generation;

        let second = atlas.get_or_insert(key, &fonts);

        assert_eq!(second, Some(first));
        assert_eq!(atlas.generation, generation);
        assert_eq!(atlas.glyphs.len(), 1);
    }

    /// Asserts that a character the primary face lacks is drawn from the
    /// matching CJK fallback at the key's font size.
    ///
    /// Case: a shell prints Japanese text at the default font size.
    #[test]
    fn a_cjk_glyph_is_drawn_from_the_matching_fallback_at_the_key_size() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let fallback = fonts.fallback_choice(&FontFace::Regular);
        assert_eq!(fonts.choice(&FontFace::Regular).glyph_id('あ'), 0);
        assert_ne!(fallback.glyph_id('あ'), 0);
        let expected = standalone_raster(fallback, 'あ', 24);
        let key = GlyphKey::new(FontFace::Regular, u32::from('あ'), 24);

        let rect = atlas.get_or_insert(key, &fonts).expect("'あ' rasterizes");

        assert_sprite_is(&atlas, rect, &expected);
        assert_eq!(i32::from(rect.offset_x), expected.placement.left);
    }

    /// Asserts that a character only the symbol face carries rasterizes to a
    /// non-empty sprite.
    ///
    /// Case: an interactive TUI draws a checked checkbox in a multi-select
    /// list.
    #[test]
    fn a_symbol_only_glyph_rasterizes_through_the_symbol_face() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        assert_eq!(fonts.choice(&FontFace::Regular).glyph_id('☑'), 0);
        assert_eq!(fonts.fallback_choice(&FontFace::Regular).glyph_id('☑'), 0);
        assert_ne!(fonts.symbol.glyph_id('☑'), 0);
        let key = GlyphKey::new(FontFace::Regular, u32::from('☑'), 24);

        let rect = atlas.get_or_insert(key, &fonts).expect("'☑' rasterizes");

        assert!(rect.w > 0 && rect.h > 0, "{rect:?}");
    }

    /// Asserts that a codepoint outside the Unicode scalar values returns
    /// `None` and packs nothing.
    ///
    /// Case: a corrupted cell hands the renderer a lone surrogate or a value
    /// past U+10FFFF.
    #[test]
    fn a_codepoint_outside_the_unicode_scalars_returns_none() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let generation = atlas.generation;

        for codepoint in [0xD800, 0xDFFF, 0x11_0000] {
            let key = GlyphKey::new(FontFace::Regular, codepoint, 24);
            assert_eq!(atlas.get_or_insert(key, &fonts), None, "U+{codepoint:04X}");
        }

        assert!(atlas.glyphs.is_empty());
        assert_eq!(atlas.generation, generation);
    }

    /// Asserts that a valid codepoint no face in the fallback chain carries
    /// returns `None` and packs nothing.
    ///
    /// Case: a program prints a noncharacter that no bundled font draws.
    #[test]
    fn a_codepoint_no_face_carries_returns_none() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let ch = '\u{1FFFE}';
        assert_eq!(fonts.choice(&FontFace::Regular).glyph_id(ch), 0);
        assert_eq!(fonts.fallback_choice(&FontFace::Regular).glyph_id(ch), 0);
        assert_eq!(fonts.symbol.glyph_id(ch), 0);
        let generation = atlas.generation;

        let result =
            atlas.get_or_insert(GlyphKey::new(FontFace::Regular, u32::from(ch), 24), &fonts);

        assert_eq!(result, None);
        assert!(atlas.glyphs.is_empty());
        assert_eq!(atlas.generation, generation);
    }

    /// Asserts that a glyph with zero extent returns `None` and is not packed.
    ///
    /// Case: a shell prints the spaces between two words.
    #[test]
    fn a_zero_extent_glyph_returns_none() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from(' '), 24);

        assert_eq!(atlas.get_or_insert(key, &fonts), None);
        assert!(!atlas.glyphs.contains_key(&key));
    }

    /// Asserts that a sprite exactly as large as the atlas is accepted rather
    /// than refused.
    ///
    /// Case: a glyph is requested in an atlas sized to hold exactly that one
    /// sprite.
    #[test]
    fn a_sprite_exactly_the_atlas_size_is_accepted() {
        let fonts = TerminalFonts::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let (w, h) = sprite_size(key, &fonts);
        let mut atlas = GlyphAtlas::new(w, h);

        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("a sprite the atlas's own size fits");

        assert_eq!((u32::from(rect.w), u32::from(rect.h)), (w, h));
    }

    /// Asserts that a sprite larger than the atlas on either axis returns
    /// `None`, packs nothing, and does not restart the atlas.
    ///
    /// Case: a glyph is requested at a size the configured atlas cannot hold.
    #[test]
    fn a_sprite_larger_than_the_atlas_on_either_axis_returns_none() {
        let fonts = TerminalFonts::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let (w, h) = sprite_size(key, &fonts);

        for (width, height) in [(w - 1, h), (w, h - 1)] {
            let mut atlas = GlyphAtlas::new(width, height);
            assert_eq!(atlas.get_or_insert(key, &fonts), None, "{width}x{height}");
            assert!(atlas.glyphs.is_empty(), "{width}x{height}");
            assert_eq!(atlas.restarts, 0, "{width}x{height}");
        }
    }

    /// Asserts that a glyph arriving at a full atlas clears every packed
    /// glyph, lands at the top-left, and bumps both counters, and that an
    /// evicted key rasterizes again on its next lookup.
    ///
    /// Case: a long session has filled the atlas and a letter not yet packed
    /// appears on screen.
    #[test]
    fn a_full_atlas_clears_every_glyph_and_restarts_at_the_top_left() {
        let fonts = TerminalFonts::default();
        let a = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let b = GlyphKey::new(FontFace::Regular, u32::from('B'), 24);
        let (wa, ha) = sprite_size(a, &fonts);
        let (wb, hb) = sprite_size(b, &fonts);
        let mut atlas = GlyphAtlas::new(wa.max(wb), ha.max(hb));
        atlas.get_or_insert(a, &fonts).expect("'A' rasterizes");
        let restarts = atlas.restarts;
        let generation = atlas.generation;

        let rect = atlas
            .get_or_insert(b, &fonts)
            .expect("'B' rasterizes after the restart");

        assert_eq!((rect.u, rect.v), (0, 0));
        assert_eq!(atlas.glyphs.len(), 1);
        assert!(atlas.glyphs.contains_key(&b));
        assert!(atlas.restarts > restarts);
        assert_ne!(atlas.generation, generation);
        let generation = atlas.generation;
        atlas
            .get_or_insert(a, &fonts)
            .expect("'A' rasterizes again");
        assert_ne!(atlas.generation, generation);
    }

    /// Asserts that a glyph that exactly fills the space left beside or below
    /// the packed glyph is packed without restarting the atlas.
    ///
    /// Case: the atlas has just enough room left for one more sprite.
    #[test]
    fn a_glyph_that_exactly_fills_the_remaining_space_does_not_restart() {
        let fonts = TerminalFonts::default();
        let a = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let b = GlyphKey::new(FontFace::Regular, u32::from('B'), 24);
        let (wa, ha) = sprite_size(a, &fonts);
        let (wb, hb) = sprite_size(b, &fonts);

        for (width, height) in [(wa + wb, ha.max(hb)), (wa.max(wb), ha + hb)] {
            let mut atlas = GlyphAtlas::new(width, height);
            atlas.get_or_insert(a, &fonts).expect("'A' rasterizes");

            let rect = atlas.get_or_insert(b, &fonts);

            assert!(rect.is_some(), "{width}x{height}");
            assert_eq!(atlas.restarts, 0, "{width}x{height}");
            assert!(atlas.glyphs.contains_key(&a), "{width}x{height}");
            assert!(atlas.glyphs.contains_key(&b), "{width}x{height}");
        }
    }

    /// Asserts that a mark is composed onto its glyph in an entry of its own,
    /// whose box contains the bare glyph's box, is taller, and keeps the bare
    /// glyph's coverage.
    ///
    /// Case: a shell echoes `e` followed by a combining acute accent.
    #[test]
    fn a_mark_is_composed_onto_its_glyph_in_an_entry_of_its_own() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        assert_ne!(fonts.choice(&FontFace::Regular).glyph_id('\u{0301}'), 0);
        let bare = GlyphKey::new(FontFace::Regular, u32::from('e'), 24);
        let base = atlas.get_or_insert(bare, &fonts).expect("'e' rasterizes");

        let marked = atlas
            .get_or_insert(bare.with_marks(['\u{0301}']), &fonts)
            .expect("'e' with an accent rasterizes");

        let dx = usize::try_from(base.offset_x - marked.offset_x)
            .expect("the composed box starts at or left of the base");
        let dy = usize::try_from(base.offset_y - marked.offset_y)
            .expect("the composed box starts at or above the base");
        assert!(
            dx + usize::from(base.w) <= usize::from(marked.w),
            "{marked:?} vs {base:?}"
        );
        assert!(
            dy + usize::from(base.h) <= usize::from(marked.h),
            "{marked:?} vs {base:?}"
        );
        assert!(marked.h > base.h, "{marked:?} vs {base:?}");
        let base_pixels = sprite_pixels(&atlas, base);
        let marked_pixels = sprite_pixels(&atlas, marked);
        for y in 0..usize::from(base.h) {
            for x in 0..usize::from(base.w) {
                let alpha = base_pixels[y * usize::from(base.w) + x];
                let composed = marked_pixels[(y + dy) * usize::from(marked.w) + x + dx];
                assert!(composed >= alpha, "({x}, {y})");
            }
        }
        assert_eq!(atlas.glyphs.len(), 2);
    }

    /// Asserts that a mark whose outline is empty is left out, leaving the
    /// bare glyph's sprite.
    ///
    /// Case: a cell's marks include a zero width space, which the face
    /// carries as a glyph with no outline.
    #[test]
    fn a_mark_that_outlines_to_nothing_is_left_out() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let face = fonts.choice(&FontFace::Regular);
        let zwsp = face.glyph_id('\u{200B}');
        assert_ne!(zwsp, 0);
        let mut context = ScaleContext::new();
        let mut scaler = context.builder(**face).size(24.0).hint(false).build();
        let bounds = scaler
            .scale_outline(zwsp)
            .expect("U+200B has an outline record")
            .bounds();
        assert!(
            bounds.width() == 0.0 || bounds.height() == 0.0,
            "U+200B outline is {} x {}",
            bounds.width(),
            bounds.height()
        );
        let bare = GlyphKey::new(FontFace::Regular, u32::from('e'), 24);
        let base = atlas.get_or_insert(bare, &fonts).expect("'e' rasterizes");

        let marked = atlas
            .get_or_insert(bare.with_marks(['\u{200B}']), &fonts)
            .expect("'e' with an empty mark rasterizes");

        assert_same_sprite(&atlas, marked, base);
    }

    /// Asserts that a mark placed clear of its glyph is left out, leaving the
    /// bare glyph's sprite.
    ///
    /// Case: a program prints an ideographic comma followed by a combining
    /// grave accent.
    #[test]
    fn a_mark_placed_clear_of_its_glyph_is_left_out() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        assert_eq!(fonts.choice(&FontFace::Regular).glyph_id('、'), 0);
        assert_ne!(
            fonts
                .fallback_choice(&FontFace::Regular)
                .glyph_id('\u{0300}'),
            0
        );
        let bare = GlyphKey::new(FontFace::Regular, u32::from('、'), 24);
        let base = atlas.get_or_insert(bare, &fonts).expect("、 rasterizes");

        let marked = atlas
            .get_or_insert(bare.with_marks(['\u{0300}']), &fonts)
            .expect("、 with a grave accent rasterizes");

        assert_same_sprite(&atlas, marked, base);
    }

    /// Asserts that a glyph resolved through the symbol face takes no marks,
    /// even a mark the symbol face carries.
    ///
    /// Case: a TUI draws a checked checkbox followed by a combining enclosing
    /// keycap.
    #[test]
    fn a_symbol_face_glyph_takes_no_marks() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        assert_ne!(fonts.symbol.glyph_id('\u{20E3}'), 0);
        let bare = GlyphKey::new(FontFace::Regular, 0x2611, 24);
        let base = atlas.get_or_insert(bare, &fonts).expect("☑ rasterizes");

        let marked = atlas
            .get_or_insert(bare.with_marks(['\u{20E3}']), &fonts)
            .expect("☑ with a keycap rasterizes");

        assert_same_sprite(&atlas, marked, base);
    }

    /// Asserts that packing a second glyph into an atlas with room leaves the
    /// first glyph's rect and pixels intact and does not restart the atlas.
    ///
    /// Case: a shell prints two different letters in a fresh session.
    #[test]
    fn a_second_glyph_leaves_the_first_intact() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let a = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let b = GlyphKey::new(FontFace::Regular, u32::from('B'), 24);
        let first = atlas.get_or_insert(a, &fonts).expect("'A' rasterizes");
        let first_pixels = sprite_pixels(&atlas, first);

        let second = atlas.get_or_insert(b, &fonts).expect("'B' rasterizes");

        assert_eq!(sprite_pixels(&atlas, first), first_pixels);
        assert_eq!(atlas.glyphs.get(&a), Some(&first));
        assert_eq!(atlas.glyphs.get(&b), Some(&second));
        let apart = first.u + first.w <= second.u
            || second.u + second.w <= first.u
            || first.v + first.h <= second.v
            || second.v + second.h <= first.v;
        assert!(apart, "{first:?} overlaps {second:?}");
        assert_eq!(atlas.restarts, 0);
    }

    /// Asserts that the same character at two font sizes gets two sprites,
    /// the larger size the taller one.
    ///
    /// Case: the user zooms in, so the renderer requests every glyph at a
    /// larger size.
    #[test]
    fn the_same_character_at_two_sizes_gets_two_sprites() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();

        let small = atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('A'), 24), &fonts)
            .expect("'A' at 24 px rasterizes");
        let large = atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('A'), 48), &fonts)
            .expect("'A' at 48 px rasterizes");

        assert_eq!(atlas.glyphs.len(), 2);
        assert!(large.h > small.h, "{large:?} vs {small:?}");
    }

    /// Asserts that a bold key draws with the bold primary face, and with the
    /// bold CJK fallback for a character the primary lacks.
    ///
    /// Case: a program prints bold text that mixes Latin and Japanese.
    #[test]
    fn a_bold_key_draws_with_the_bold_faces() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let cases = [
            (
                'A',
                fonts.choice(&FontFace::Bold),
                fonts.choice(&FontFace::Regular),
            ),
            (
                'あ',
                fonts.fallback_choice(&FontFace::Bold),
                fonts.fallback_choice(&FontFace::Regular),
            ),
        ];

        for (ch, bold, regular) in cases {
            let expected = standalone_raster(bold, ch, 24);
            assert_ne!(
                expected.data,
                standalone_raster(regular, ch, 24).data,
                "{ch}"
            );

            let rect = atlas
                .get_or_insert(GlyphKey::new(FontFace::Bold, u32::from(ch), 24), &fonts)
                .expect("a bold glyph rasterizes");

            assert_sprite_is(&atlas, rect, &expected);
        }
    }

    /// Asserts that a sprite larger than a full atlas returns `None` without
    /// clearing the glyphs already packed.
    ///
    /// Case: the user zooms in far enough that one glyph outgrows an atlas
    /// already full of glyphs at the old size.
    #[test]
    fn an_oversized_sprite_does_not_clear_a_populated_atlas() {
        let fonts = TerminalFonts::default();
        let small = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let (w, h) = sprite_size(small, &fonts);
        let mut atlas = GlyphAtlas::new(w, h);
        atlas
            .get_or_insert(small, &fonts)
            .expect("'A' at 24 px rasterizes");

        let result =
            atlas.get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('A'), 96), &fonts);

        assert_eq!(result, None);
        assert!(atlas.glyphs.contains_key(&small));
        assert_eq!(atlas.restarts, 0);
    }
}
