use ab_glyph::{Font, OutlinedGlyph, ScaleFont};
use bevy::{platform::collections::HashMap, prelude::*};

use crate::glyph::font::{GlyphKey, TerminalFonts};

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
        let font = fonts.choice(&key.face);
        let ch = char::from_u32(key.codepoint)?;
        let scale = ab_glyph::PxScale::from(fonts.px_scale_value(key.size_px));
        let scaled = font.as_scaled(scale);
        let glyph_id = scaled.glyph_id(ch);
        // NOTE: ab_glyph maps unknown codepoints to glyph ID 0 (notdef), and
        //       outline_glyph happily returns the notdef rectangle ("tofu")
        //       outline if the font has one — which most do. Bail out before
        //       rasterizing so combining marks, CJK glyphs missing from the
        //       monospace face, etc. do not leave a literal tofu in every
        //       cell. The bundled JetBrains Mono Nerd Font Mono includes PUA
        //       icons (U+E000–U+F8FF used by neo-tree, nvim-web-devicons,
        //       lazygit), so those codepoints DO map to non-zero glyph IDs
        //       and DO rasterize through this path. A user-supplied non-
        //       Nerd-Font override will fall back to notdef here (and
        //       short-circuit) for any PUA codepoints, which is the
        //       desired behavior for monospace fonts without icon coverage.
        if glyph_id.0 == 0 {
            return None;
        }

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

    /// Clears the atlas — used in tests and after font-config changes.
    pub fn clear(&mut self) {
        self.pixels.fill(0);
        self.glyphs.clear();
        self.shelves.clear();
        self.generation = self.generation.wrapping_add(1);
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

impl Shelf {
    pub fn reset(&mut self) {
        self.x = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::font::{FontFace, GlyphKey, TerminalFonts};

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
}
