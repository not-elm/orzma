//! The renderer's CPU-side mirror of one terminal: the `TerminalGrid`
//! component, the cells it materializes from a frame's runs, and the
//! cursor and hover queries the host reads off it.

use crate::schema::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, GridColumn,
    GridLine, GridPoint, Hyperlink, HyperlinkId, HyperlinkUri, Palette, Run, SelectionRange,
    ViCursor,
};
use bevy::prelude::*;
use orzma_vt::prelude::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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
    /// each applied frame's `hyperlinks`; a known id is never
    /// overwritten. Linear scan — realistic sessions carry ≤100
    /// distinct hyperlinks (mirroring the server-side interner
    /// rationale).
    pub hyperlinks: Vec<(HyperlinkId, HyperlinkUri)>,
    /// The live palette set by the last frame that carried one;
    /// symbolic cell colors resolve against it.
    pub palette: Palette,
    /// Webview placements in active-grid coordinates, mirrored from the
    /// last applied frame. Replaced wholesale by every frame that
    /// carries a list — absence from the list means "no live anchor
    /// this frame".
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

    /// Whether applying `frame` would change this grid.
    ///
    /// A frame is self-describing about change — an absent row and a
    /// `None` section mean "unchanged" — so this reads only what
    /// [`Self::apply`] would write: a row inside the grid, a new size,
    /// a moved cursor or viewport, a listed section that differs, or a
    /// hyperlink id the table lacks. Rows beyond the grid and known
    /// hyperlink ids do not count.
    ///
    /// # Invariants
    ///
    /// Returns `true` exactly when [`Self::apply`] mutates something.
    /// The observer derefs the component mutably only on `true`, so a
    /// spurious `true` here rebuilds the GPU buffers for nothing and a
    /// spurious `false` drops a repaint.
    pub fn differs_from(&self, frame: &Frame) -> bool {
        // NOTE: This list enumerates the same fields `apply` writes; a
        // field added to one without the other either drops repaints
        // or rebuilds the GPU buffers for nothing.
        let rows_in_range = frame
            .rows
            .iter()
            .any(|row| usize::from(row.line.0) < self.cells.len());
        let size_differs = frame.size.cols != self.cols || frame.size.rows != self.rows;
        let cursor_differs = self.cursor != Some(frame.cursor)
            || self.display_offset != frame.display_offset.0
            || self.vi_cursor != frame.vi_cursor
            || self.selection != frame.selection;
        let placements_differ = frame
            .placements
            .as_ref()
            .is_some_and(|placements| *placements != self.placements);
        let palette_differs = frame
            .palette
            .as_ref()
            .is_some_and(|palette| *palette != self.palette);
        let new_hyperlinks = frame
            .hyperlinks
            .iter()
            .any(|link| !self.knows_hyperlink(link.id));
        rows_in_range
            || size_differs
            || cursor_differs
            || placements_differ
            || palette_differs
            || new_hyperlinks
    }

    /// Applies `frame` to this grid.
    ///
    /// The size is settled first so every row the frame carries has a
    /// slot, the hyperlink table is merged before rows are materialized
    /// so they resolve against it, and the `None` sections are left
    /// alone. Row damage is not diffed against the cells already there:
    /// a row's presence is the VT's statement that it changed.
    ///
    /// # Invariants
    ///
    /// After `apply` returns, `self.cells.len() == self.rows as usize`.
    /// [`Self::differs_from`] relies on this invariant when it reads
    /// `self.cells.len()` to decide whether a row is in range, which it
    /// does before this method has run the resize for the frame under
    /// consideration.
    pub fn apply(&mut self, frame: &Frame) {
        // NOTE: Keep the fields written here in step with the list
        // `differs_from` reads, for the reason its NOTE gives.
        if frame.size.cols != self.cols || frame.size.rows != self.rows {
            self.cols = frame.size.cols;
            self.rows = frame.size.rows;
            self.cells
                .resize_with(usize::from(frame.size.rows), Vec::new);
        }
        for link in &frame.hyperlinks {
            if !self.knows_hyperlink(link.id) {
                self.hyperlinks.push((link.id, link.uri.clone()));
            }
        }
        for row in &frame.rows {
            let index = usize::from(row.line.0);
            if index < self.cells.len() {
                let line = GridLine(i32::from(row.line.0) - frame.display_offset.0 as i32);
                self.cells[index] = runs_to_cells(&row.contents, line, &self.hyperlinks);
            }
        }
        self.cursor = Some(frame.cursor);
        self.display_offset = frame.display_offset.0;
        self.vi_cursor = frame.vi_cursor;
        self.selection = frame.selection;
        if let Some(placements) = &frame.placements {
            self.placements.clone_from(placements);
        }
        if let Some(palette) = &frame.palette {
            self.palette = palette.clone();
        }
    }

    fn knows_hyperlink(&self, id: HyperlinkId) -> bool {
        self.hyperlinks.iter().any(|(known, _)| *known == id)
    }
}

/// Materializes one row's attribute runs into cells, resolving each
/// run's hyperlink id against the retained table.
///
/// Column advance follows display width — a wide grapheme takes two
/// columns and a combining mark none — which `material::rebuild_cells`
/// mirrors.
fn runs_to_cells(
    runs: &[Run],
    line: GridLine,
    hyperlinks: &[(HyperlinkId, HyperlinkUri)],
) -> Vec<GridCell> {
    let mut out: Vec<GridCell> = Vec::new();
    let mut column: u16 = 0;
    for run in runs {
        let hyperlink = run.hyperlink_id.and_then(|id| {
            hyperlinks
                .iter()
                .find(|(known, _)| *known == id)
                .map(|(id, uri)| Hyperlink {
                    id: *id,
                    uri: uri.clone(),
                })
        });
        for grapheme in run.text.graphemes(true) {
            let w = grapheme.width();
            let width = if w >= 2 {
                2u8
            } else if w == 0 {
                0
            } else {
                1
            };
            out.push(GridCell {
                text: grapheme.to_string(),
                width,
                point: GridPoint {
                    line,
                    column: GridColumn(column),
                },
                fg: run.fg,
                bg: run.bg,
                style: run.style.bits(),
                hyperlink: hyperlink.clone(),
            });
            column = column.saturating_add(u16::from(width));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Color, Cursor, CursorShape, GridColumn, GridLine, GridPoint, Hyperlink, PlacementId,
        PlacementSize, Rgb, Row, Style,
    };
    use orzma_vt::prelude::{DirtyRow, GridSize, ViewportLine};

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

    fn run_with_link(text: &str, hyperlink_id: Option<HyperlinkId>) -> Run {
        Run {
            cols: 1,
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Style::empty(),
            text: text.to_string(),
            hyperlink_id,
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

    /// Asserts that a run's hyperlink id resolves against the retained
    /// table when cells are built, and that an id absent from the
    /// table leaves the cell unlinked.
    ///
    /// Case: a shell prints an OSC 8 link whose id → URI entry arrived
    /// in an earlier frame's table.
    #[test]
    fn runs_to_cells_resolves_hyperlink_ids_against_the_table() {
        let runs = vec![
            run_with_link("a", Some(HyperlinkId(7))),
            run_with_link("b", Some(HyperlinkId(9))),
        ];
        let table = vec![(HyperlinkId(7), HyperlinkUri::new("https://example"))];
        let cells = runs_to_cells(&runs, GridLine(0), &table);
        assert_eq!(
            cells[0].hyperlink.as_ref().map(|h| h.id),
            Some(HyperlinkId(7))
        );
        assert_eq!(
            cells[0].hyperlink.as_ref().map(|h| h.uri.as_str()),
            Some("https://example")
        );
        assert!(cells[1].hyperlink.is_none());
    }

    /// Asserts that cell points carry the given line and a column walk
    /// that advances by display width.
    ///
    /// Case: a row mixes a wide CJK grapheme with ASCII text on a
    /// scrolled-back history line, so the ASCII cell's point must land
    /// after both columns of the wide character.
    #[test]
    fn runs_to_cells_assigns_points_by_display_width() {
        let cells = runs_to_cells(&[run_with_link("あb", None)], GridLine(-3), &[]);
        assert_eq!(
            cells[0].point,
            GridPoint {
                line: GridLine(-3),
                column: GridColumn(0),
            }
        );
        assert_eq!(
            cells[1].point,
            GridPoint {
                line: GridLine(-3),
                column: GridColumn(2),
            }
        );
    }

    /// A frame for a one-by-one grid that changes nothing on its own:
    /// no rows, `None` sections, the default cursor at offset zero.
    fn quiet_frame() -> Frame {
        Frame {
            size: GridSize { cols: 1, rows: 1 },
            rows: vec![],
            cursor: Cursor::default(),
            display_offset: DisplayOffset(0),
            vi_cursor: None,
            selection: None,
            placements: None,
            palette: None,
            hyperlinks: vec![],
        }
    }

    /// A grid that already mirrors [`quiet_frame`].
    fn settled_grid() -> TerminalGrid {
        TerminalGrid {
            cols: 1,
            rows: 1,
            cells: vec![vec![]],
            cursor: Some(Cursor::default()),
            ..Default::default()
        }
    }

    fn dirty_row(line: u16, text: &str) -> DirtyRow {
        DirtyRow {
            line: ViewportLine(line),
            contents: Row::from(vec![run_with_link(text, None)]),
        }
    }

    /// Asserts that a frame carrying nothing new reports no difference.
    ///
    /// Case: a frame's every section already equals what the mirror
    /// holds, because the coalescer folded in a change the mirror had
    /// already settled to before this frame reached it.
    #[test]
    fn a_quiet_frame_does_not_differ() {
        assert!(!settled_grid().differs_from(&quiet_frame()));
    }

    /// Asserts that a moved cursor is a difference, and that applying
    /// the frame settles the grid so the cursor round-trips to a
    /// matching state.
    ///
    /// Case: the user presses an arrow key and the application moves
    /// the caret without repainting a cell.
    #[test]
    fn a_moved_cursor_differs() {
        let mut grid = settled_grid();
        let frame = Frame {
            cursor: Cursor {
                point: GridPoint {
                    line: GridLine(1),
                    column: GridColumn(2),
                },
                ..Cursor::default()
            },
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert!(!grid.differs_from(&frame));
    }

    /// Asserts that a moved viewport (display offset) is a difference,
    /// and that applying the frame settles the grid so the offset
    /// round-trips to a matching state.
    ///
    /// Case: the user scrolls back through history without the shell
    /// repainting any cell.
    #[test]
    fn a_moved_viewport_differs() {
        let mut grid = settled_grid();
        let frame = Frame {
            display_offset: DisplayOffset(7),
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert!(!grid.differs_from(&frame));
    }

    /// Asserts that a size change alone is a difference, and that
    /// applying the frame resizes the cell rows to match even though
    /// the frame carries no rows of its own.
    ///
    /// Case: the user resizes the window and the VT's first frame at
    /// the new size arrives before any output repaints a row.
    #[test]
    fn a_new_size_alone_differs() {
        let mut grid = settled_grid();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!(grid.cells.len(), 2);
        assert!(!grid.differs_from(&frame));
    }

    /// Asserts that a row inside the grid is applied at the line the
    /// display offset projects it to, and counts as a difference.
    ///
    /// Case: a build prints one line while the user is scrolled back
    /// three rows.
    #[test]
    fn a_row_in_range_is_applied_at_its_projected_line() {
        let mut grid = TerminalGrid {
            cols: 2,
            rows: 2,
            cells: vec![vec![], vec![]],
            cursor: Some(Cursor::default()),
            display_offset: 3,
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(1, "x")],
            display_offset: DisplayOffset(3),
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!(grid.cells[1][0].text, "x");
        assert_eq!(grid.cells[1][0].point.line, GridLine(-2));
        assert!(grid.cells[0].is_empty());
    }

    /// Asserts that a row beyond the grid is ignored and is not a
    /// difference.
    ///
    /// Case: a malformed frame names a row past the mirror's last row.
    #[test]
    fn a_row_out_of_range_is_ignored() {
        let mut grid = settled_grid();
        let frame = Frame {
            rows: vec![dirty_row(5, "x")],
            ..quiet_frame()
        };
        assert!(!grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!(grid.cells.len(), 1);
    }

    /// Asserts that a size change resizes the cell rows before the
    /// frame's rows are applied.
    ///
    /// Case: the user drags the window taller and the VT's next frame
    /// carries every row of the new size.
    #[test]
    fn a_size_change_resizes_the_cells() {
        let mut grid = settled_grid();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!((grid.cols, grid.rows), (3, 2));
        assert_eq!(grid.cells.len(), 2);
        assert_eq!(grid.cells[1][0].text, "b");
    }

    /// Asserts that a placements list replaces the mirror wholesale,
    /// including down to empty, and that `None` leaves it alone.
    ///
    /// Case: a webview scrolls out of the viewport, so the next frame
    /// carries an empty list, and the frame after that carries no list
    /// because nothing moved.
    #[test]
    fn placements_replace_wholesale_and_none_keeps_them() {
        let mut grid = settled_grid();
        let placed = AnchoredPlacement {
            id: PlacementId(1),
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 4, cols: 5 },
        };
        let mounted = Frame {
            placements: Some(vec![placed]),
            ..quiet_frame()
        };
        assert!(grid.differs_from(&mounted));
        grid.apply(&mounted);
        assert_eq!(grid.placements, vec![placed]);

        let unchanged = quiet_frame();
        assert!(!grid.differs_from(&unchanged));
        grid.apply(&unchanged);
        assert_eq!(grid.placements, vec![placed]);

        let hidden = Frame {
            placements: Some(vec![]),
            ..quiet_frame()
        };
        assert!(grid.differs_from(&hidden));
        grid.apply(&hidden);
        assert_eq!(grid.placements, vec![]);
    }

    /// Asserts that a palette replaces the mirror and `None` keeps it.
    ///
    /// Case: OSC 11 recolors the background once, and every later frame
    /// carries no palette.
    #[test]
    fn a_palette_replaces_the_mirror_and_none_keeps_it() {
        let mut grid = settled_grid();
        let palette = Palette {
            background: Rgb { r: 9, g: 8, b: 7 },
            ..Palette::default()
        };
        let recolored = Frame {
            palette: Some(palette),
            ..quiet_frame()
        };
        assert!(grid.differs_from(&recolored));
        grid.apply(&recolored);
        assert_eq!(grid.palette.background, Rgb { r: 9, g: 8, b: 7 });
        assert!(!grid.differs_from(&quiet_frame()));
    }

    /// Asserts that hyperlinks merge without overwriting a known id,
    /// and that a known id alone is not a difference.
    ///
    /// Case: a program re-announces a link id it already defined, with
    /// a different URI, alongside a genuinely new one.
    #[test]
    fn hyperlinks_merge_without_overwrite() {
        let mut grid = TerminalGrid {
            hyperlinks: vec![(HyperlinkId(1), HyperlinkUri::new("https://old"))],
            ..settled_grid()
        };
        let repeated = Frame {
            hyperlinks: vec![Hyperlink {
                id: HyperlinkId(1),
                uri: HyperlinkUri::new("https://CHANGED"),
            }],
            ..quiet_frame()
        };
        assert!(!grid.differs_from(&repeated));

        let extended = Frame {
            hyperlinks: vec![
                Hyperlink {
                    id: HyperlinkId(1),
                    uri: HyperlinkUri::new("https://CHANGED"),
                },
                Hyperlink {
                    id: HyperlinkId(2),
                    uri: HyperlinkUri::new("https://new"),
                },
            ],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&extended));
        grid.apply(&extended);
        assert_eq!(grid.hyperlinks.len(), 2);
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://old");
        assert_eq!(grid.hyperlinks[1].1.as_str(), "https://new");
    }

    /// Asserts that a row resolves a hyperlink id defined by an earlier
    /// frame's table.
    ///
    /// Case: a link's definition arrived in one frame and the row that
    /// references it is repainted in a later one.
    #[test]
    fn a_row_resolves_a_hyperlink_from_an_earlier_frame() {
        let mut grid = settled_grid();
        grid.apply(&Frame {
            hyperlinks: vec![Hyperlink {
                id: HyperlinkId(4),
                uri: HyperlinkUri::new("https://earlier"),
            }],
            ..quiet_frame()
        });
        grid.apply(&Frame {
            rows: vec![DirtyRow {
                line: ViewportLine(0),
                contents: Row::from(vec![run_with_link("a", Some(HyperlinkId(4)))]),
            }],
            ..quiet_frame()
        });
        assert_eq!(
            grid.cells[0][0].hyperlink.as_ref().map(|h| h.uri.as_str()),
            Some("https://earlier")
        );
    }
}
