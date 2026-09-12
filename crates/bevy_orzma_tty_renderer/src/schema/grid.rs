//! The renderer's CPU-side mirror of one terminal, materialized into
//! cells from the frames applied to it.

use crate::schema::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, Hyperlink,
    HyperlinkId, HyperlinkUri, Palette, Run, SelectionRange, ViCursor,
};
use bevy::prelude::*;
use orzma_vt::prelude::Frame;
#[cfg(test)]
use orzma_vt::prelude::GridSize;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// One materialized cell of the renderer's CPU-side grid, expanded
/// from the frame's [`crate::schema::Run`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    /// The grapheme cluster text for this cell.
    pub text: String,
    /// Display width: 2 for wide CJK, 1 otherwise.
    pub width: u8,
    /// Foreground color, symbolic.
    pub fg: Color,
    /// Background color, symbolic.
    pub bg: Color,
    /// Style bitmask, carried over unchanged from [`crate::schema::Run::style`].
    pub style: u16,
    /// Hyperlink resolved from the grid's retained table, if any.
    pub hyperlink: Option<Hyperlink>,
}

impl GridCell {
    /// Whether this cell paints no glyph: its text is empty or all
    /// whitespace.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// One column of the renderer's CPU-side grid.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum GridSlot {
    /// No run covered this column.
    #[default]
    Empty,
    /// The grapheme cluster occupying this column.
    Cell(GridCell),
    /// The right half of the wide cell in the preceding column.
    WideTrailer,
}

impl GridSlot {
    /// The cell painting this column, or `None` for a column no run
    /// covered and for a wide cell's right half.
    #[inline]
    pub fn cell(&self) -> Option<&GridCell> {
        match self {
            Self::Cell(cell) => Some(cell),
            Self::Empty | Self::WideTrailer => None,
        }
    }
}

/// The layout structure of the terminal grid.
/// Each terminal entity owns this component.
#[derive(Component, Default)]
pub struct TerminalGrid {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Cell grid indexed `[row][col]`.
    pub cells: Vec<Vec<GridSlot>>,
    /// Current cursor state, absent until the first frame arrives.
    pub cursor: Option<Cursor>,
    /// Lines scrolled back from the live tail; 0 = at live tail.
    pub display_offset: u32,
    /// Vi-mode cursor from the last applied frame; `None` when that frame
    /// carries none.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range from the last applied frame, independent of
    /// `vi_cursor`.
    pub selection: Option<SelectionRange>,
    /// App-level cursor visibility override. When `true`,
    /// `current_cursor_pos_and_style()` clears [`CURSOR_VISIBLE_BIT`]
    /// before returning. It is independent of `Cursor.visible`, so it
    /// hides the cursor without clobbering terminal-controlled state.
    pub suppress_cursor: bool,
    /// OSC 8 hyperlinks indexed by id. Every applied frame merges its
    /// `hyperlinks` into this table, and a known id is never
    /// overwritten.
    pub hyperlinks: Vec<(HyperlinkId, HyperlinkUri)>,
    /// The live palette set by the last frame that carried one;
    /// symbolic cell colors resolve against it.
    pub palette: Palette,
    /// Webview placements in active-grid coordinates, mirrored from the
    /// last applied frame. Every frame that carries a list replaces
    /// this one wholesale, and a placement absent from that list has no
    /// live anchor this frame.
    pub placements: Vec<AnchoredPlacement>,
}

impl TerminalGrid {
    /// Resolves `(row, col)` to the hyperlink at that visible cell, if
    /// any. `col` is a column coordinate: both columns of a wide cell
    /// resolve to the same hyperlink. Returns `None` for out-of-bounds
    /// or unlinked cells.
    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<(HyperlinkId, &HyperlinkUri)> {
        let row_slots = self.cells.get(usize::from(row))?;
        let col = usize::from(col);
        let cell = match row_slots.get(col)? {
            GridSlot::Cell(cell) => cell,
            GridSlot::WideTrailer => row_slots.get(col.checked_sub(1)?)?.cell()?,
            GridSlot::Empty => return None,
        };
        let link = cell.hyperlink.as_ref()?;
        Some((link.id, &link.uri))
    }

    /// Projects the cursor into viewport cells as `(column, row)`, or
    /// `None` when no frame has arrived yet or the cursor's line is
    /// scrolled out of the visible rows.
    pub fn cursor_viewport_cell(&self) -> Option<(u16, u16)> {
        let cursor = self.cursor.as_ref()?;
        let row = cursor
            .point
            .line
            .to_viewport(DisplayOffset(self.display_offset), self.rows)?;
        Some((cursor.point.column.0, row.0))
    }

    /// Projects the cursor into viewport cells as `(column, row)`, but
    /// never yields `None`: a cursor whose line is scrolled out of the
    /// visible rows keeps its column on row `0`, and a missing cursor maps
    /// to the origin.
    pub fn cursor_viewport_cell_or_top(&self) -> (u16, u16) {
        let Some(cursor) = self.cursor.as_ref() else {
            return (0, 0);
        };
        let row = cursor
            .point
            .line
            .to_viewport(DisplayOffset(self.display_offset), self.rows)
            .map_or(0, |row| row.0);
        (cursor.point.column.0, row)
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
    /// An absent row and a `None` section mean "unchanged", so this counts
    /// only a row inside the frame's own size, a size the grid does not
    /// hold yet, a cursor, vi cursor, selection or viewport that differs,
    /// a listed section that differs, or a hyperlink id the table lacks.
    /// Rows beyond the frame's size and known hyperlink ids do not
    /// count.
    ///
    /// # Invariants
    ///
    /// Returns `true` exactly when [`Self::apply`] mutates something.
    pub fn differs_from(&self, frame: &Frame) -> bool {
        let Frame {
            size,
            rows,
            cursor,
            display_offset,
            vi_cursor,
            selection,
            placements,
            palette,
            hyperlinks,
        } = frame;
        size.cols != self.cols
            || size.rows != self.rows
            || self.cells.len() != usize::from(size.rows)
            || self.cursor != Some(*cursor)
            || self.display_offset != display_offset.0
            || self.vi_cursor != *vi_cursor
            || self.selection != *selection
            || rows.iter().any(|row| row.line.0 < size.rows)
            || placements
                .as_ref()
                .is_some_and(|placements| *placements != self.placements)
            || palette
                .as_ref()
                .is_some_and(|palette| *palette != self.palette)
            || hyperlinks.iter().any(|link| !self.knows_hyperlink(link.id))
    }

    /// Applies `frame` to this grid.
    ///
    /// Every row the frame carries inside its size replaces the cells at
    /// that line, resolving its hyperlink ids against the table, into
    /// which this frame's own definitions are merged first; the `None`
    /// sections are left alone.
    ///
    /// # Invariants
    ///
    /// After `apply` returns, `self.cells.len() == self.rows as usize`,
    /// whatever length the grid was built with. It mutates the grid
    /// exactly when [`Self::differs_from`] reports `true`.
    pub fn apply(&mut self, frame: &Frame) {
        let Frame {
            size,
            rows,
            cursor,
            display_offset,
            vi_cursor,
            selection,
            placements,
            palette,
            hyperlinks,
        } = frame;
        self.cols = size.cols;
        self.rows = size.rows;
        let cols = usize::from(size.cols);
        self.cells
            .resize_with(usize::from(size.rows), || vec![GridSlot::Empty; cols]);
        for row in &mut self.cells {
            if row.len() != cols {
                row.clear();
                row.resize(cols, GridSlot::Empty);
            }
        }
        for link in hyperlinks {
            if !self.knows_hyperlink(link.id) {
                self.hyperlinks.push((link.id, link.uri.clone()));
            }
        }
        for row in rows {
            let Some(slot) = self.cells.get_mut(usize::from(row.line.0)) else {
                continue;
            };
            *slot = runs_to_cells(&row.contents, size.cols, &self.hyperlinks);
        }
        self.cursor = Some(*cursor);
        self.display_offset = display_offset.0;
        self.vi_cursor = *vi_cursor;
        self.selection = *selection;
        if let Some(placements) = placements {
            self.placements.clone_from(placements);
        }
        if let Some(palette) = palette {
            self.palette.clone_from(palette);
        }
    }

    fn knows_hyperlink(&self, id: HyperlinkId) -> bool {
        lookup_hyperlink(&self.hyperlinks, id).is_some()
    }
}

/// Finds the URI the retained table holds for `id`.
fn lookup_hyperlink(
    table: &[(HyperlinkId, HyperlinkUri)],
    id: HyperlinkId,
) -> Option<&HyperlinkUri> {
    table
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, uri)| uri)
}

/// Materializes one row's attribute runs into exactly `cols` column
/// slots, resolving each run's hyperlink id against the retained table.
///
/// A wide grapheme takes a cell slot plus the [`GridSlot::WideTrailer`]
/// that follows it; a zero-width grapheme takes no column and is
/// dropped. Runs that do not fill the row leave [`GridSlot::Empty`]
/// behind, and content past the last column is truncated.
fn runs_to_cells(
    runs: &[Run],
    cols: u16,
    hyperlinks: &[(HyperlinkId, HyperlinkUri)],
) -> Vec<GridSlot> {
    let width = usize::from(cols);
    let mut out = vec![GridSlot::Empty; width];
    let mut column = 0usize;
    for run in runs {
        let hyperlink = run.hyperlink_id.and_then(|id| {
            lookup_hyperlink(hyperlinks, id).map(|uri| Hyperlink {
                id,
                uri: uri.clone(),
            })
        });
        for grapheme in run.text.graphemes(true) {
            let cell_width = grapheme.width().min(2) as u8;
            if cell_width == 0 {
                continue;
            }
            if column >= width {
                return out;
            }
            out[column] = GridSlot::Cell(GridCell {
                text: grapheme.to_string(),
                width: cell_width,
                fg: run.fg,
                bg: run.bg,
                style: run.style.bits(),
                hyperlink: hyperlink.clone(),
            });
            column += 1;
            if cell_width == 2 && column < width {
                out[column] = GridSlot::WideTrailer;
                column += 1;
            }
        }
    }
    out
}

#[cfg(test)]
impl TerminalGrid {
    /// A one-by-one grid that already mirrors [`quiet_frame`].
    pub(crate) fn settled() -> Self {
        Self {
            cols: 1,
            rows: 1,
            cells: vec![vec![GridSlot::Empty]],
            cursor: Some(Cursor::default()),
            ..Default::default()
        }
    }
}

/// A frame for a one-by-one grid that changes nothing on its own: no
/// rows, `None` sections, the default cursor at offset zero.
#[cfg(test)]
pub(crate) fn quiet_frame() -> Frame {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Color, Cursor, CursorShape, GridColumn, GridLine, GridPoint, Hyperlink, InstanceId,
        PlacementSize, Rgb, Row, Style,
    };
    use orzma_vt::prelude::{DirtyRow, ViewportLine};

    fn cell_with_link(text: &str, width: u8, link: Option<(u32, &str)>) -> GridCell {
        GridCell {
            text: text.to_string(),
            width,
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

    /// Asserts that `cursor_viewport_cell` follows the display offset and
    /// yields `None` once the cursor's line scrolls out of view.
    ///
    /// Case: the user scrolls back through history while the shell's
    /// cursor sits on the bottom row of a 24-row screen.
    #[test]
    fn cursor_viewport_cell_follows_the_display_offset() {
        let mut grid = TerminalGrid {
            cols: 80,
            rows: 24,
            ..TerminalGrid::default()
        };
        grid.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(23),
                column: GridColumn(5),
            },
            ..Cursor::default()
        });
        assert_eq!(grid.cursor_viewport_cell(), Some((5, 23)));

        grid.display_offset = 3;
        assert_eq!(grid.cursor_viewport_cell(), None);

        grid.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(10),
                column: GridColumn(0),
            },
            ..Cursor::default()
        });
        assert_eq!(grid.cursor_viewport_cell(), Some((0, 13)));
    }

    /// Asserts that the `_or_top` fallback keeps the cursor's column on row
    /// `0` when the line is scrolled out of view, and yields the origin when
    /// no cursor exists.
    ///
    /// Case: the user scrolls back through history during an IME
    /// composition, pushing the prompt's cursor row out of the viewport.
    #[test]
    fn cursor_viewport_cell_or_top_keeps_the_column_when_off_viewport() {
        let mut grid = TerminalGrid {
            cols: 80,
            rows: 24,
            ..TerminalGrid::default()
        };
        assert_eq!(grid.cursor_viewport_cell_or_top(), (0, 0));

        grid.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(23),
                column: GridColumn(5),
            },
            ..Cursor::default()
        });
        grid.display_offset = 3;
        assert_eq!(grid.cursor_viewport_cell_or_top(), (5, 0));
    }

    /// Asserts that suppression clears the visible bit of the packed
    /// style.
    ///
    /// Case: an IME composition temporarily hides the caret.
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
    /// Case: an IME composition hides the caret while a projected
    /// cursor position is already recorded on the mirror.
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
    /// paints nothing rather than being clamped to an edge cell it does
    /// not occupy.
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

    /// Asserts that a column no run covered resolves to no hyperlink.
    ///
    /// Case: the pointer hovers a column of the row that no attribute
    /// run painted this frame.
    #[test]
    fn hyperlink_at_returns_none_for_empty_slot() {
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![GridSlot::Empty; 4]],
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
            cells: vec![vec![GridSlot::Cell(cell)]],
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
            cells: vec![vec![GridSlot::Cell(cell)]],
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
            cells: vec![vec![
                GridSlot::Cell(wide_linked),
                GridSlot::WideTrailer,
                GridSlot::Cell(trailing),
            ]],
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
        let slots = runs_to_cells(&runs, 2, &table);
        assert_eq!(
            slots[0]
                .cell()
                .and_then(|c| c.hyperlink.as_ref())
                .map(|h| h.id),
            Some(HyperlinkId(7))
        );
        assert_eq!(
            slots[0]
                .cell()
                .and_then(|c| c.hyperlink.as_ref())
                .map(|h| h.uri.as_str()),
            Some("https://example")
        );
        assert!(
            slots[1]
                .cell()
                .expect("run fills every slot")
                .hyperlink
                .is_none()
        );
    }

    /// Asserts that a row materializes one slot per column, with a wide
    /// grapheme taking a cell slot and the trailer slot that follows it.
    ///
    /// Case: a CJK character is printed at the start of a four-column
    /// row.
    #[test]
    fn runs_to_cells_indexes_slots_by_column() {
        let slots = runs_to_cells(&[run_with_link("あz", None)], 4, &[]);
        assert_eq!(slots.len(), 4);
        assert_eq!(slots[0].cell().map(|c| c.text.as_str()), Some("あ"));
        assert_eq!(slots[0].cell().map(|c| c.width), Some(2));
        assert_eq!(slots[1], GridSlot::WideTrailer);
        assert_eq!(slots[2].cell().map(|c| c.text.as_str()), Some("z"));
        assert_eq!(slots[3], GridSlot::Empty);
    }

    /// Asserts that a combining mark inside a grapheme cluster shares
    /// its base character's column instead of taking one of its own.
    ///
    /// Case: a program prints an accented latin word, so the accent
    /// arrives inside the same cluster as the letter it modifies.
    #[test]
    fn runs_to_cells_keeps_a_combining_cluster_in_one_column() {
        let slots = runs_to_cells(&[run_with_link("a\u{0301}b", None)], 3, &[]);
        assert_eq!(slots[0].cell().map(|c| c.text.as_str()), Some("a\u{0301}"));
        assert_eq!(slots[1].cell().map(|c| c.text.as_str()), Some("b"));
        assert_eq!(slots[2], GridSlot::Empty);
    }

    /// Asserts that a grapheme whose display width is zero is dropped
    /// rather than given a column of its own.
    ///
    /// Case: a run boundary splits a cluster, so the trailing combining
    /// mark arrives as a run of its own.
    #[test]
    fn runs_to_cells_drops_a_standalone_combining_mark() {
        let runs = vec![
            run_with_link("a", None),
            run_with_link("\u{0301}", None),
            run_with_link("b", None),
        ];
        let slots = runs_to_cells(&runs, 3, &[]);
        assert_eq!(slots[0].cell().map(|c| c.text.as_str()), Some("a"));
        assert_eq!(slots[1].cell().map(|c| c.text.as_str()), Some("b"));
        assert_eq!(slots[2], GridSlot::Empty);
    }

    fn dirty_row(line: u16, text: &str) -> DirtyRow {
        DirtyRow {
            line: ViewportLine(line),
            contents: Row::from(vec![run_with_link(text, None)]),
        }
    }

    /// Asserts that a frame carrying nothing new reports no difference.
    ///
    /// Case: a frame repeats what the mirror already holds.
    #[test]
    fn a_quiet_frame_does_not_differ() {
        assert!(!TerminalGrid::settled().differs_from(&quiet_frame()));
    }

    /// Asserts that a moved cursor is a difference, and that applying
    /// the frame settles the grid so the cursor round-trips to a
    /// matching state.
    ///
    /// Case: the user presses an arrow key and the application moves
    /// the caret without repainting a cell.
    #[test]
    fn a_moved_cursor_differs() {
        let mut grid = TerminalGrid::settled();
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
    /// Case: a frame that only moves the display offset reaches a settled
    /// mirror.
    #[test]
    fn a_moved_viewport_differs() {
        let mut grid = TerminalGrid::settled();
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
    /// Case: a frame that only changes the size reaches a settled mirror.
    #[test]
    fn a_new_size_alone_differs() {
        let mut grid = TerminalGrid::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!(grid.cells.len(), 2);
        assert!(!grid.differs_from(&frame));
    }

    /// Asserts that a row inside the grid replaces that row's contents
    /// when applied, while a row the frame does not touch keeps its
    /// full length of empty slots, and counts as a difference.
    ///
    /// Case: a build prints one line of a two-row pane.
    #[test]
    fn a_row_in_range_is_applied_at_its_projected_line() {
        let mut grid = TerminalGrid {
            cols: 2,
            rows: 2,
            cells: vec![vec![], vec![]],
            cursor: Some(Cursor::default()),
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(1, "x")],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!(grid.cells[1][0].cell().map(|c| c.text.as_str()), Some("x"));
        assert_eq!(grid.cells[0], vec![GridSlot::Empty; 2]);
    }

    /// Asserts that a grid built at the frame's size but without cell
    /// rows still differs, and that applying the frame gives every row
    /// a slot and fills the carried ones.
    ///
    /// Case: the host pre-sizes the grid to the PTY geometry before the
    /// VT's bootstrap repaint arrives at that same size.
    #[test]
    fn a_pre_sized_grid_without_cells_takes_the_frame_rows() {
        let mut grid = TerminalGrid {
            cols: 2,
            rows: 2,
            cursor: Some(Cursor::default()),
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!(grid.cells.len(), 2);
        assert_eq!(grid.cells[1][0].cell().map(|c| c.text.as_str()), Some("b"));
        assert!(!grid.differs_from(&Frame {
            size: GridSize { cols: 2, rows: 2 },
            ..quiet_frame()
        }));
    }

    /// Asserts that a frame with fewer rows than the grid truncates the
    /// cell rows to the new height.
    ///
    /// Case: the user drags the window shorter and the VT's repaint at
    /// the new height arrives.
    #[test]
    fn a_shrinking_size_truncates_the_cells() {
        let mut grid = TerminalGrid {
            cols: 1,
            rows: 3,
            cells: vec![vec![], vec![], vec![]],
            cursor: Some(Cursor::default()),
            ..Default::default()
        };
        let frame = Frame {
            rows: vec![dirty_row(0, "a")],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!((grid.cols, grid.rows), (1, 1));
        assert_eq!(grid.cells.len(), 1);
        assert_eq!(grid.cells[0][0].cell().map(|c| c.text.as_str()), Some("a"));
    }

    /// Asserts that a frame changing only the column count is a
    /// difference and repaints the rows it carries at the new width.
    ///
    /// Case: the user drags the window wider without changing its
    /// height, and the VT's repaint at the new width arrives.
    #[test]
    fn a_cols_only_size_change_differs_and_repaints() {
        let mut grid = TerminalGrid::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 1 },
            rows: vec![dirty_row(0, "abc")],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!((grid.cols, grid.rows), (3, 1));
        assert_eq!(grid.cells[0].len(), 3);
        assert!(!grid.differs_from(&Frame {
            size: GridSize { cols: 3, rows: 1 },
            ..quiet_frame()
        }));
    }

    /// Asserts that a row beyond the frame's own size is ignored and is
    /// not a difference.
    ///
    /// Case: a malformed frame names a row past its own last row.
    #[test]
    fn a_row_out_of_range_is_ignored() {
        let mut grid = TerminalGrid::settled();
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
        let mut grid = TerminalGrid::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
            ..quiet_frame()
        };
        assert!(grid.differs_from(&frame));
        grid.apply(&frame);
        assert_eq!((grid.cols, grid.rows), (3, 2));
        assert_eq!(grid.cells.len(), 2);
        assert_eq!(grid.cells[1][0].cell().map(|c| c.text.as_str()), Some("b"));
    }

    /// Asserts that a placements list replaces the mirror wholesale,
    /// including down to empty, and that `None` leaves it alone.
    ///
    /// Case: a program mounts a webview, a quiet frame follows, and the
    /// program then unmounts it, so the next frame carries an empty
    /// list.
    #[test]
    fn placements_replace_wholesale_and_none_keeps_them() {
        let mut grid = TerminalGrid::settled();
        let placed = AnchoredPlacement {
            id: InstanceId(1),
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
    /// Case: one frame recolors the background once, and every later
    /// frame carries no palette.
    #[test]
    fn a_palette_replaces_the_mirror_and_none_keeps_it() {
        let mut grid = TerminalGrid::settled();
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
    /// Case: a later frame re-sends a definition the mirror already
    /// holds alongside a genuinely new one.
    #[test]
    fn hyperlinks_merge_without_overwrite() {
        let mut grid = TerminalGrid {
            hyperlinks: vec![(HyperlinkId(1), HyperlinkUri::new("https://old"))],
            ..TerminalGrid::settled()
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
        let mut grid = TerminalGrid::settled();
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
            grid.cells[0][0]
                .cell()
                .and_then(|c| c.hyperlink.as_ref())
                .map(|h| h.uri.as_str()),
            Some("https://earlier")
        );
    }
}
