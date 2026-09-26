//! The live palette pre-packed into the shader's linear color encoding.

use crate::material::pack_linear;
use orzma_vt::prelude::{Color as CellColor, Palette};

/// The transparent cell-background packing (`alpha == 0`) the shader
/// treats as "terminal default background".
pub const TRANSPARENT_BG: u32 = 0;

/// The grid palette pre-packed to the shader's linear `u32` encoding.
pub struct PackedPalette {
    indexed: [u32; 256],
    foreground: u32,
    background: u32,
}

impl PackedPalette {
    /// Packs each color slot of `palette` once.
    pub fn build(palette: &Palette) -> Self {
        Self {
            indexed: palette.indexed.map(pack_linear),
            foreground: pack_linear(palette.foreground),
            background: pack_linear(palette.background),
        }
    }

    /// Packs a cell foreground, resolving symbolic colors to their
    /// palette slot.
    //
    // NOTE: The variant-to-slot mapping mirrors `Palette::resolve` in
    //       `orzma_vt`, pre-packed here for the per-cell hot path; a
    //       change to either mapping must be applied to both, or
    //       symbolic colors silently diverge between producers.
    pub fn cell_fg(&self, color: CellColor) -> u32 {
        match color {
            CellColor::DefaultForeground => self.foreground,
            CellColor::DefaultBackground => self.background,
            CellColor::Indexed(index) => self.indexed[usize::from(index)],
            CellColor::Rgb(rgb) => pack_linear(rgb),
        }
    }

    /// Packs a cell background.
    ///
    /// # Invariants
    ///
    /// `DefaultBackground` packs [`TRANSPARENT_BG`], never the opaque
    /// palette background, so an explicit RGB equal to that background
    /// stays distinguishable from the default.
    pub fn cell_bg(&self, color: CellColor) -> u32 {
        match color {
            CellColor::DefaultBackground => TRANSPARENT_BG,
            other => self.cell_fg(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that fg and bg packing resolve symbolic colors through
    /// the live palette, and that the default background packs the
    /// transparent sentinel instead of the palette value.
    ///
    /// Case: a palette whose default foreground and indexed slot 1
    /// already hold custom colors packs cells for a webview overlay
    /// mounted behind default-background cells.
    #[test]
    fn cell_packing_resolves_through_the_live_palette() {
        use orzma_vt::prelude::Rgb;
        let mut palette = Palette {
            foreground: Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
            ..Palette::default()
        };
        palette.indexed[1] = Rgb {
            r: 40,
            g: 50,
            b: 60,
        };
        let packed = PackedPalette::build(&palette);
        assert_eq!(
            packed.cell_fg(CellColor::DefaultForeground),
            pack_linear(Rgb {
                r: 10,
                g: 20,
                b: 30,
            })
        );
        assert_eq!(
            packed.cell_fg(CellColor::Indexed(1)),
            pack_linear(Rgb {
                r: 40,
                g: 50,
                b: 60,
            })
        );
        assert_eq!(packed.cell_bg(CellColor::DefaultBackground), TRANSPARENT_BG);
        assert_ne!(
            packed.cell_bg(CellColor::Rgb(palette.background)),
            TRANSPARENT_BG,
            "an explicit RGB equal to the palette background must stay opaque"
        );
    }
}
