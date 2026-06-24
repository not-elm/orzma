use ab_glyph::{Font, FontArc, OutlinedGlyph};
use bevy::{platform::collections::HashMap, prelude::*};

use crate::glyph::font::{FontFace, GlyphKey, TerminalFonts};

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
/// Packs glyphs using shelf packing (rows of uniform height per shelf).
/// When the atlas is full, clears and restarts from the top-left — Tier 1
/// keeps this simple because realistic monospaced workloads (a few hundred
/// ASCII + CJK characters) never fill a 1024×1024 atlas during normal use.
#[derive(Resource)]
pub struct GlyphAtlas {
    /// One byte of alpha coverage per pixel, row-major.
    pub pixels: Vec<u8>,
    /// All glyphs currently packed into the atlas.
    pub glyphs: HashMap<GlyphKey, GlyphRect>,
    /// Bumped on every rasterization and on `clear`. The render plugin
    /// re-uploads the GPU texture when this value changes.
    pub generation: u64,
    shelves: Shelves,
}

/// Resolves a glyph for the requested codepoint, preferring the primary face
/// and falling back to `fallback_choice` when the primary's `glyph_id` is 0
/// (notdef).
///
/// Returns `(font, glyph_id, used_fallback)` for the resolved face, or `None`
/// when neither primary nor fallback contains the glyph. The `used_fallback`
/// flag tells `get_or_insert` which em-matched `PxScale` to rasterize at: the
/// primary's (`px_scale_value`) or the fallback's (`fallback_px_scale_value`),
/// so each face renders its em-square at the same physical pixel size.
///
/// `glyph_id` lookup is scale-independent, so this resolves before any scale is
/// chosen.
///
/// NOTE: retries on `glyph_id == 0` only — NOT on degenerate outline
/// (`w == 0 || h == 0`), which `get_or_insert` still short-circuits after
/// outlining. PUA Nerd Font icons (U+E000–U+F8FF) resolve non-zero on the
/// primary, so they never reach the fallback.
fn resolve_glyph<'a>(
    fonts: &'a TerminalFonts,
    face: &FontFace,
    ch: char,
) -> Option<(&'a FontArc, ab_glyph::GlyphId, bool)> {
    let primary = fonts.choice(face);
    let id = primary.glyph_id(ch);
    if id.0 != 0 {
        return Some((primary, id, false));
    }
    let fallback = fonts.fallback_choice(face);
    let id = fallback.glyph_id(ch);
    if id.0 != 0 {
        return Some((fallback, id, true));
    }
    None
}

impl GlyphAtlas {
    /// Creates an empty atlas with the given pixel dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            pixels: vec![0; (width * height) as usize],
            glyphs: HashMap::new(),
            generation: 0,
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
    /// Returns `None` when `face` is out of range, the codepoint is not a
    /// valid Unicode scalar, or the glyph has zero extent (e.g. ASCII space,
    /// combining marks, or glyphs the font does not carry).
    pub fn get_or_insert(&mut self, key: GlyphKey, fonts: &TerminalFonts) -> Option<GlyphRect> {
        if let Some(r) = self.glyphs.get(&key) {
            return Some(*r);
        }
        let ch = char::from_u32(key.codepoint)?;
        let (font, glyph_id, used_fallback) = resolve_glyph(fonts, &key.face, ch)?;
        let scale_value = if used_fallback {
            fonts.fallback_px_scale_value(key.size_px)
        } else {
            fonts.px_scale_value(key.size_px)
        };
        let scale = ab_glyph::PxScale::from(scale_value);

        let outlined = font.outline_glyph(glyph_id.with_scale(scale))?;
        let bounds = outlined.px_bounds();
        let w = bounds.width().ceil() as u16;
        let h = bounds.height().ceil() as u16;
        if w == 0 || h == 0 {
            return None;
        }

        self.shelves.new_line_if_need(w);
        if self.shelves.would_overflow() {
            self.shelves.clear();
            self.pixels.fill(0);
            self.glyphs.clear();
        }
        let u = self.shelves.shelf.x as u16;
        let v = self.shelves.y as u16;
        self.write_outline_pixels(&outlined);
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

    fn write_outline_pixels(&mut self, outlined: &OutlinedGlyph) {
        let u = self.shelves.shelf.x;
        let v = self.shelves.y;
        let atlas_width = self.shelves.width as usize;
        let atlas_height = self.shelves.height as usize;
        outlined.draw(|px, py, alpha| {
            let xx = u as usize + px as usize;
            let yy = v as usize + py as usize;
            if xx < atlas_width && yy < atlas_height {
                self.pixels[yy * atlas_width + xx] = (alpha * 255.0) as u8;
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

    #[inline]
    pub const fn would_overflow(&self) -> bool {
        self.height < self.y + self.shelf.height
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
    use crate::glyph::font::{FontFace, GlyphKey, TerminalFonts};

    fn make_key(face: FontFace, codepoint: u32, size_px: u16) -> GlyphKey {
        GlyphKey {
            face,
            codepoint,
            size_px,
        }
    }

    #[test]
    fn returned_rect_matches_written_pixels() {
        let mut atlas = GlyphAtlas::new(256, 256);
        let fonts = TerminalFonts::default();
        let key = GlyphKey {
            face: FontFace::Regular,
            codepoint: 'A' as u32,
            size_px: 24,
        };

        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("ASCII glyph should rasterize");
        assert_eq!(rect.u, 0, "first glyph must start at the left edge");
        assert_eq!(rect.v, 0, "first glyph must start at the top edge");

        let has_ink = (rect.v as u32..(rect.v as u32 + rect.h as u32)).any(|y| {
            (rect.u as u32..(rect.u as u32 + rect.w as u32)).any(|x| {
                let idx = (y * atlas.width() + x) as usize;
                atlas.pixels[idx] > 0
            })
        });
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
        let key = make_key(FontFace::Regular, u32::from('a'), 24);
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
        let key = make_key(FontFace::Regular, 0x3042, 24);
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
        let key = make_key(FontFace::Regular, 0xE0B0, 24);
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
        let key = make_key(FontFace::Regular, 0x3042, size); // 'あ'
        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("'あ' must rasterize via fallback");

        // Height the buggy code produced: the fallback outline at the PRIMARY
        // scale. The fix must rasterize 'あ' strictly smaller than this.
        let fb = fonts.fallback_choice(&FontFace::Regular);
        let primary_scale = ab_glyph::PxScale::from(fonts.px_scale_value(size));
        let gid = fb.glyph_id('あ');
        let buggy_h = fb
            .outline_glyph(gid.with_scale(primary_scale))
            .expect("'あ' outline at primary scale")
            .px_bounds()
            .height();

        assert!(
            (rect.h as f32) < buggy_h - 0.5,
            "'あ' rect height {} must be smaller than primary-scaled height {buggy_h}",
            rect.h
        );
    }

    #[test]
    fn unknown_codepoint_returns_none() {
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        // U+1FFFFE — Plane 1 unassigned, not in either font.
        let key = make_key(FontFace::Regular, 0x1FFFFE, 24);
        let result = atlas.get_or_insert(key, &fonts);
        assert!(
            result.is_none(),
            "unknown codepoint must return None (tofu suppression)"
        );
    }
}
