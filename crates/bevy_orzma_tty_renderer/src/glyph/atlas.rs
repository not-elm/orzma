//! The CPU-side glyph atlas: rasterized sprites packed onto shelves of one
//! coverage texture.

use crate::font::TerminalFonts;
use crate::glyph::{
    GlyphKey,
    outline::{GlyphTier, fit_symbol_to_cell, outline_marks, resolve_glyph, union},
};
use ab_glyph::{Font, OutlinedGlyph, PxScale};
use bevy::{platform::collections::HashMap, prelude::*};
use std::iter::once;

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
        let scale_value = match tier {
            GlyphTier::Primary => fonts.px_scale_value(key.size_px),
            GlyphTier::Fallback => fonts.fallback_px_scale_value(key.size_px),
            GlyphTier::Symbol => fonts.symbol_px_scale_value(key.size_px),
        };
        let scale = PxScale::from(scale_value);

        let outlined = font.outline_glyph(glyph_id.with_scale(scale))?;
        let outlined = if matches!(tier, GlyphTier::Symbol) {
            fit_symbol_to_cell(
                font,
                glyph_id,
                scale_value,
                outlined,
                fonts.cell_advance_px(key.size_px),
            )
        } else {
            outlined
        };
        let marks = if matches!(tier, GlyphTier::Symbol) {
            Vec::new()
        } else {
            outline_marks(font, scale, &outlined, glyph_id, key)
        };
        let bounds = marks
            .iter()
            .map(OutlinedGlyph::px_bounds)
            .fold(outlined.px_bounds(), union);
        let w = bounds.width().ceil() as u16;
        let h = bounds.height().ceil() as u16;
        if w == 0 || h == 0 || u32::from(w) > self.width() || u32::from(h) > self.height() {
            return None;
        }

        self.shelves.new_line_if_need(w);
        if self.shelves.would_overflow(h) {
            self.shelves.clear();
            self.pixels.fill(0);
            self.glyphs.clear();
            self.restarts = self.restarts.wrapping_add(1);
        }
        let u = self.shelves.shelf.x as u16;
        let v = self.shelves.y as u16;
        for glyph in once(&outlined).chain(&marks) {
            let local = glyph.px_bounds();
            let dx = (local.min.x - bounds.min.x) as u32;
            let dy = (local.min.y - bounds.min.y) as u32;
            self.write_outline_pixels(glyph, dx, dy);
        }
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

    /// Blends `outlined`'s coverage into the current shelf position,
    /// shifted right by `dx` and down by `dy` pixels, keeping the higher
    /// coverage where pixels overlap.
    fn write_outline_pixels(&mut self, outlined: &OutlinedGlyph, dx: u32, dy: u32) {
        let u = self.shelves.shelf.x + dx;
        let v = self.shelves.y + dy;
        let atlas_width = self.shelves.width as usize;
        let atlas_height = self.shelves.height as usize;
        outlined.draw(|px, py, alpha| {
            let xx = u as usize + px as usize;
            let yy = v as usize + py as usize;
            if xx < atlas_width && yy < atlas_height {
                let coverage = (alpha * 255.0) as u8;
                let slot = &mut self.pixels[yy * atlas_width + xx];
                *slot = (*slot).max(coverage);
            }
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontFace;

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
    fn cjk_rasterizes_at_fallback_scale_not_primary_scale() {
        use ab_glyph::Font as _;
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let size = 24u16;
        let key = GlyphKey::new(FontFace::Regular, u32::from('あ'), size);
        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("'あ' must rasterize via fallback");

        let fb = fonts.fallback_choice(&FontFace::Regular);
        let primary_scale = PxScale::from(fonts.px_scale_value(size));
        let gid = fb.glyph_id('あ');
        let primary_scaled_h = fb
            .outline_glyph(gid.with_scale(primary_scale))
            .expect("'あ' outline at primary scale")
            .px_bounds()
            .height();

        assert!(
            (rect.h as f32) < primary_scaled_h - 0.5,
            "'あ' rect height {} must be smaller than the primary-scaled height {primary_scaled_h}",
            rect.h
        );
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
        let cell_w = fonts.cell_metrics_px(size).advance_phys;
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

    /// Rasterizes `outlined` on its own, as a `(w, h, pixels)` triple in
    /// the same layout as [`sprite_pixels`].
    fn rasterize_alone(outlined: &OutlinedGlyph) -> (u16, u16, Vec<u8>) {
        let bounds = outlined.px_bounds();
        let w = bounds.width().ceil() as usize;
        let h = bounds.height().ceil() as usize;
        let mut pixels = vec![0u8; w * h];
        outlined.draw(|px, py, alpha| {
            pixels[py as usize * w + px as usize] = (alpha * 255.0) as u8;
        });
        (w as u16, h as u16, pixels)
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

    /// Asserts that a bare glyph's sprite is pixel for pixel the glyph
    /// rasterized on its own.
    ///
    /// Case: ordinary ASCII text is drawn.
    #[test]
    fn a_bare_glyph_matches_its_standalone_raster() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let key = GlyphKey::new(FontFace::Regular, u32::from('A'), 24);
        let rect = atlas.get_or_insert(key, &fonts).expect("'A' rasterizes");
        let font = fonts.choice(&FontFace::Regular);
        let scale = PxScale::from(fonts.px_scale_value(24));
        let outlined = font
            .outline_glyph(font.glyph_id('A').with_scale(scale))
            .expect("'A' outlines");
        let (w, h, pixels) = rasterize_alone(&outlined);
        assert_eq!((rect.w, rect.h), (w, h));
        assert_eq!(sprite_pixels(&atlas, rect), pixels);
        let bounds = outlined.px_bounds();
        assert_eq!(rect.offset_x, bounds.min.x.floor() as i16);
        assert_eq!(rect.offset_y, bounds.min.y.floor() as i16);
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
}
