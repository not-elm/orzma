//! Tests for the per-pane cell upload, one file per operation under test.

use super::*;
use crate::font::FontFace;
use orzma_vt::prelude::Cell;

mod upload;
mod upload_terminal_cells;

/// One row holding a plain cell for each `char` of `text`.
fn row_of(text: &str) -> TerminalCells {
    TerminalCells {
        cells: vec![
            text.chars()
                .map(|c| Cell {
                    c,
                    ..Cell::default()
                })
                .collect(),
        ],
        ..Default::default()
    }
}

/// A 32x24 atlas whose one shelf already holds 24 px `M` and `W`, so a
/// 24 px `A` restarts it.
fn nearly_full_atlas(fonts: &TerminalFonts) -> GlyphAtlas {
    let mut atlas = GlyphAtlas::new(32, 24);
    for ch in ['M', 'W'] {
        atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from(ch), 24), fonts)
            .expect("the filler glyph rasterizes");
    }
    assert_eq!(
        atlas.restarts, 0,
        "the filler glyphs must not restart the atlas"
    );
    atlas
}
