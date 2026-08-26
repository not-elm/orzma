use crate::schema::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, GridPoint,
    Hyperlink, HyperlinkId, HyperlinkUri, Palette, SelectionRange, ViCursor,
};
use bevy::prelude::*;

/// One materialized cell of the renderer's CPU-side grid, expanded
/// from the frame's [`crate::schema::Run`]s.
///
/// This is renderer vocabulary, not part of the VT contract: the VT
/// emits attribute runs, and the renderer materializes them into cells
/// for glyph resolution and hover hit-testing.
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    /// The grapheme cluster text for this cell.
    pub text: String,
    /// Display width: 2 for wide CJK, 0 for combining marks, 1 otherwise.
    pub width: u8,
    /// Active-grid coordinates of the cell.
    pub point: GridPoint,
    /// Foreground color, symbolic.
    pub fg: Color,
    /// Background color, symbolic.
    pub bg: Color,
    /// Style bitmask, carried over unchanged from [`crate::schema::Run::style`].
    pub style: u16,
    /// Hyperlink resolved from the frame's interner table, if any.
    pub hyperlink: Option<Hyperlink>,
}

impl GridCell {
    /// Whether this cell paints no glyph: a zero-width cell (combining mark /
    /// wide-char spacer) or one whose text is empty or all whitespace.
    ///
    /// Shared by the renderer's glyph resolution and the host paint-rescue's
    /// blank-grid test so the two notions of "renders nothing" cannot drift.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.width == 0 || self.text.trim().is_empty()
    }
}

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
    /// The live palette from the last applied snapshot; symbolic cell
    /// colors resolve against it. Replaced on snapshot only.
    pub palette: Palette,
    /// Webview placements in active-grid coordinates, mirrored from the
    /// last applied frame. Replaced wholesale on snapshot AND delta —
    /// absence from the list means "no live anchor this frame".
    pub placements: Vec<AnchoredPlacement>,
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

    /// Returns the viewport cursor cell and the packed style the
    /// shader decodes, preferring the vi cursor over the live cursor.
    ///
    /// A cursor whose grid point projects outside the viewport (the
    /// user has scrolled it away) paints nothing: both the position
    /// and the packed style stay zero.
    pub fn current_cursor_pos_and_style(&self) -> (UVec2, u32) {
        let offset = DisplayOffset(self.display_offset);
        let mut cursor_pos = UVec2::ZERO;
        let mut cursor_style = 0;
        if let Some(vc) = self.vi_cursor {
            if let Some(line) = vc.point.line.to_viewport(offset, self.rows) {
                cursor_pos = UVec2::new(u32::from(vc.point.column.0), u32::from(line.0));
                cursor_style = Cursor {
                    point: vc.point,
                    shape: CursorShape::Block,
                    blinking: false,
                    visible: true,
                }
                .pack_cursor_style();
            }
        } else if let Some(c) = self.cursor.as_ref()
            && let Some(line) = c.point.line.to_viewport(offset, self.rows)
        {
            cursor_pos = UVec2::new(u32::from(c.point.column.0), u32::from(line.0));
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
    use crate::schema::{Color, Cursor, CursorShape, GridColumn, GridLine, GridPoint, Hyperlink};

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
            point: GridPoint {
                line: GridLine(5),
                column: GridColumn(3),
            },
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    }

    /// Asserts that a visible cursor reports its viewport cell and a
    /// packed style with the visible bit set.
    ///
    /// Case: the shell sits at an ordinary prompt with the caret
    /// shown.
    #[test]
    fn current_cursor_pos_and_style_returns_packed_style_when_not_suppressed() {
        let grid = TerminalGrid {
            rows: 24,
            cursor: Some(visible_block_cursor()),
            suppress_cursor: false,
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::new(3, 5));
        assert_eq!(style & CURSOR_VISIBLE_BIT, CURSOR_VISIBLE_BIT);
    }

    /// Asserts that suppression clears the visible bit of the packed
    /// style.
    ///
    /// Case: IME composition temporarily hides the caret without
    /// touching terminal-controlled cursor state.
    #[test]
    fn current_cursor_pos_and_style_clears_visible_bit_when_suppressed() {
        let grid = TerminalGrid {
            rows: 24,
            cursor: Some(visible_block_cursor()),
            suppress_cursor: true,
            ..Default::default()
        };
        let (_pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    /// Asserts that suppression clears only the visible bit while the
    /// vi cursor's projected position is still reported.
    ///
    /// Case: the user composes IME text while vi mode is active, so the
    /// app hides the caret without discarding where it sits.
    #[test]
    fn suppress_cursor_does_not_affect_vi_cursor_position() {
        let grid = TerminalGrid {
            rows: 24,
            vi_cursor: Some(ViCursor {
                point: GridPoint {
                    line: GridLine(2),
                    column: GridColumn(7),
                },
            }),
            suppress_cursor: true,
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::new(7, 2));
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    /// Asserts that a cursor whose line projects outside the viewport
    /// paints nothing.
    ///
    /// The decided policy is to omit the caret rather than clamp it to
    /// an edge cell it does not occupy.
    ///
    /// Case: the user scrolls back through history while the shell
    /// keeps its caret on the live prompt line below the viewport.
    #[test]
    fn a_scrolled_away_cursor_paints_nothing() {
        let grid = TerminalGrid {
            rows: 24,
            display_offset: 30,
            cursor: Some(visible_block_cursor()),
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::ZERO);
        assert_eq!(style, 0);
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
