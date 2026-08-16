use crate::schema::{
    CURSOR_VISIBLE_BIT, Cursor, CursorShape, GridCell, HyperlinkId, HyperlinkUri, SelectionRange,
    ViCursor,
};
use bevy::prelude::*;

/// A structure represents the layout structure of the terminal grid.
/// Each terminal entity owns this component.
#[derive(Component, Default)]
pub struct TerminalGrid {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Cell grid indexed `[row][col_grapheme_index]`.
    pub cells: Vec<Vec<GridCell>>,
    /// Current cursor state, absent until the first frame arrives.
    pub cursor: Option<Cursor>,
    /// Lines scrolled back from the live tail; 0 = at live tail.
    pub display_offset: u32,
    /// Total scrollback history line count.
    pub history_size: u32,
    /// Cumulative trimmed-lines counter mirrored from the latest frame.
    pub history_base: u64,
    /// Monotonic sequence number of the last applied frame.
    pub last_seq: u32,
    /// Active terminal modes from the last snapshot (e.g. "mouse-sgr-1006").
    pub modes: Vec<String>,
    /// Vi-mode cursor when the server is in vi mode; `None` otherwise.
    /// `ViModePlugin` reads this every frame to drive `ViModeState::active`.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range emitted alongside `vi_cursor`. Independent of
    /// `vi_cursor` — survives motion without selection.
    pub selection: Option<SelectionRange>,
    /// App-level cursor visibility override. When `true`,
    /// `current_cursor_pos_and_style()` clears [`CURSOR_VISIBLE_BIT`]
    /// before returning. Independent of `Cursor.visible` (which mirrors
    /// DECTCEM from the wire) — this field is for the UI layer (e.g.,
    /// IME composition) to non-destructively hide the cursor without
    /// clobbering terminal-controlled state.
    pub suppress_cursor: bool,
    /// OSC 8 hyperlinks indexed by wire id. Populated cumulatively from
    /// `FrameSnapshot.hyperlinks` / `FrameDelta.hyperlinks`. Replaced on
    /// snapshot, merged on delta. Linear scan — realistic sessions
    /// carry ≤100 distinct hyperlinks (mirroring the server-side
    /// interner rationale).
    pub hyperlinks: Vec<(HyperlinkId, HyperlinkUri)>,
    /// Terminal default background color from `FrameSnapshot.default_bg`
    /// (sourced from OSC 11). Raw `[r, g, b]` bytes; black when not set. The
    /// material uses it as the base background for default-bg cells and the
    /// padding outside the grid; an unset `[0,0,0]` is mapped to
    /// `TerminalPaddingFallback` (the theme background) by the material system.
    pub default_bg: [u8; 3],
}

impl TerminalGrid {
    /// Resolves `(row, col)` to the hyperlink at that visible cell, if
    /// any. `col` is a column coordinate, not a grapheme index — wide
    /// cells (width=2) match both of their columns, and width-0
    /// trailers are skipped without consuming a column. Returns
    /// `None` for out-of-bounds or unlinked cells.
    //
    // NOTE: `self.cells[row]` is grapheme-indexed (one entry per
    //       cluster from `runs_to_cells`), so a column-to-cell walk
    //       is required — direct `cells[row][col]` indexing would
    //       desynchronize after any wide char or width-0 trailer.
    //       Must mirror the column-advance logic in
    //       `material::rebuild_cells`.
    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<(HyperlinkId, &HyperlinkUri)> {
        let row_cells = self.cells.get(row as usize)?;
        let mut current_col: u32 = 0;
        let target = u32::from(col);
        for cell in row_cells {
            if cell.width == 0 {
                continue;
            }
            let cell_end = current_col.saturating_add(u32::from(cell.width));
            if target >= current_col && target < cell_end {
                let link = cell.hyperlink.as_ref()?;
                return Some((link.id, &link.uri));
            }
            current_col = cell_end;
        }
        None
    }

    pub fn current_cursor_pos_and_style(&self) -> (UVec2, u32) {
        let mut cursor_pos = UVec2::ZERO;
        let mut cursor_style = 0;
        if let Some(vc) = self.vi_cursor {
            if !vc.in_scrollback && vc.row >= 0 {
                cursor_pos = UVec2::new(u32::from(vc.column), vc.row as u32);
                cursor_style = Cursor {
                    x: vc.column,
                    y: vc.row.max(0) as u16,
                    shape: CursorShape::Block,
                    blinking: false,
                    visible: true,
                }
                .pack_cursor_style();
            }
        } else if let Some(c) = self.cursor.as_ref() {
            cursor_pos = UVec2::new(u32::from(c.x), u32::from(c.y));
            cursor_style = c.pack_cursor_style();
        }
        if self.suppress_cursor {
            cursor_style &= !CURSOR_VISIBLE_BIT;
        }
        (cursor_pos, cursor_style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Color, Cursor, CursorShape, GridPoint, Hyperlink};

    fn cell_with_link(text: &str, width: u8, link: Option<(u32, &str)>) -> GridCell {
        GridCell {
            text: text.to_string(),
            width,
            point: GridPoint::default(),
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: 0,
            hyperlink: link.map(|(id, uri)| Hyperlink {
                id: HyperlinkId(id),
                uri: HyperlinkUri::new(uri),
            }),
        }
    }

    fn visible_block_cursor() -> Cursor {
        Cursor {
            x: 3,
            y: 5,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    }

    #[test]
    fn current_cursor_pos_and_style_returns_packed_style_when_not_suppressed() {
        let grid = TerminalGrid {
            cursor: Some(visible_block_cursor()),
            suppress_cursor: false,
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::new(3, 5));
        assert_eq!(style & CURSOR_VISIBLE_BIT, CURSOR_VISIBLE_BIT);
    }

    #[test]
    fn current_cursor_pos_and_style_clears_visible_bit_when_suppressed() {
        let grid = TerminalGrid {
            cursor: Some(visible_block_cursor()),
            suppress_cursor: true,
            ..Default::default()
        };
        let (_pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    #[test]
    fn suppress_cursor_does_not_affect_vi_cursor_position() {
        let grid = TerminalGrid {
            vi_cursor: Some(ViCursor {
                row: 2,
                column: 7,
                in_scrollback: false,
            }),
            suppress_cursor: true,
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::new(7, 2));
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    /// Asserts that a lookup outside the populated grid returns `None`.
    ///
    /// Case: the pointer hovers over the padding beyond the last row
    /// or column while the grid is smaller than the window.
    #[test]
    fn hyperlink_at_returns_none_when_out_of_bounds() {
        let grid = TerminalGrid {
            cols: 4,
            rows: 2,
            cells: vec![vec![], vec![]],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(99, 0).is_none());
        assert!(grid.hyperlink_at(0, 99).is_none());
    }

    /// Asserts that a linked width-0 trailer cell never resolves at a
    /// column of its own.
    ///
    /// Case: a combining mark arrives as its own wire cell inside an
    /// OSC 8 link, so the trailer carries the link but occupies no
    /// column.
    #[test]
    fn hyperlink_at_returns_none_for_width_zero_trailer() {
        let cell = cell_with_link("\u{0301}", 0, Some((5, "https://example")));
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(0, 0).is_none());
    }

    /// Asserts that a linked cell resolves to its hyperlink id and URI.
    ///
    /// Case: the user hovers an OSC 8 link a shell printed, and the
    /// input layer asks which link sits under the pointer.
    #[test]
    fn hyperlink_at_returns_id_and_uri_for_linked_cell() {
        let cell = cell_with_link("x", 1, Some((7, "https://example")));
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            ..Default::default()
        };
        let (id, uri) = grid.hyperlink_at(0, 0).expect("hyperlink present");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
    }

    /// Asserts that an unlinked cell resolves to `None`.
    ///
    /// Case: the user hovers plain shell output that carries no
    /// hyperlink.
    #[test]
    fn hyperlink_at_returns_none_for_unlinked_cell() {
        let cell = cell_with_link("x", 1, None);
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(0, 0).is_none());
    }

    /// Asserts that both columns of a wide grapheme resolve to the
    /// same hyperlink while the cell after it stays unlinked.
    ///
    /// Case: a CJK character inside an OSC 8 link spans two columns,
    /// and the user may hover either half.
    #[test]
    fn hyperlink_at_resolves_both_halves_of_wide_char() {
        let wide_linked = cell_with_link("あ", 2, Some((7, "https://example")));
        let trailing = cell_with_link("b", 1, None);
        let grid = TerminalGrid {
            cols: 3,
            rows: 1,
            cells: vec![vec![wide_linked, trailing]],
            ..Default::default()
        };
        let (id, uri) = grid.hyperlink_at(0, 0).expect("left half should resolve");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
        let (id, uri) = grid.hyperlink_at(0, 1).expect("right half should resolve");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
        assert!(grid.hyperlink_at(0, 2).is_none());
    }

    /// Asserts that a width-0 trailer does not shift the columns of
    /// the cells that follow it.
    ///
    /// Case: a combining mark emitted as its own wire cell sits
    /// between a plain cell and a linked cell, and the user hovers the
    /// linked cell's column.
    #[test]
    fn hyperlink_at_skips_width_zero_trailer_in_column_walk() {
        let base = cell_with_link("a", 1, None);
        let trailer = cell_with_link("\u{0301}", 0, None);
        let linked = cell_with_link("x", 1, Some((9, "https://x")));
        let grid = TerminalGrid {
            cols: 2,
            rows: 1,
            cells: vec![vec![base, trailer, linked]],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(0, 0).is_none());
        let (id, _uri) = grid.hyperlink_at(0, 1).expect("linked cell at col 1");
        assert_eq!(id, HyperlinkId(9));
    }
}
