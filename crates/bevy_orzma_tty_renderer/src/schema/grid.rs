//! The renderer's CPU-side mirror of one terminal's viewport and painted
//! content, materialized from the frames applied to it.

use crate::schema::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, HyperlinkId,
    HyperlinkUri, Palette, Run, SelectionRange, ViCursor,
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
    /// Foreground color, symbolic.
    pub fg: Color,
    /// Background color, symbolic.
    pub bg: Color,
    /// Style bitmask, carried over unchanged from [`crate::schema::Run::style`].
    pub style: u16,
    /// Id of the hyperlink covering this cell, present only when the
    /// retained table can resolve it. The URI lives in the table, not
    /// here.
    pub hyperlink: Option<HyperlinkId>,
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
    ///
    /// Meaningful only immediately after a [`GridSlot::Cell`] holding a
    /// double-width grapheme; a producer must not emit it after a
    /// narrow cell.
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

/// The viewport geometry of one terminal, and where the user is looking
/// and pointing within it.
#[derive(Component, Default)]
#[require(TerminalCells)]
pub struct TerminalView {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Current cursor state, absent until the first frame arrives.
    pub cursor: Option<Cursor>,
    /// Vi-mode cursor from the last applied frame; `None` when that frame
    /// carries none.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range from the last applied frame, independent of
    /// `vi_cursor`.
    pub selection: Option<SelectionRange>,
    /// Lines scrolled back from the live tail; 0 = at live tail.
    pub display_offset: u32,
    /// App-level cursor visibility override. When `true`,
    /// [`Self::current_cursor_pos_and_style`] clears
    /// [`CURSOR_VISIBLE_BIT`] before returning. It is independent of
    /// `Cursor.visible`, so it hides the cursor without clobbering
    /// terminal-controlled state.
    pub suppress_cursor: bool,
    /// Webview placements in active-grid coordinates, mirrored from the
    /// last applied frame. Every frame that carries a list replaces
    /// this one wholesale, and a placement absent from that list has no
    /// live anchor this frame.
    pub placements: Vec<AnchoredPlacement>,
}

impl TerminalView {
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

    /// Whether applying `frame` would change this view.
    ///
    /// A `None` section means "unchanged", so a `None` placements list
    /// does not count.
    ///
    /// # Invariants
    ///
    /// Returns `true` exactly when [`Self::apply`] mutates something.
    pub fn differs_from(&self, frame: &Frame) -> bool {
        let Frame {
            size,
            rows: _,
            cursor,
            display_offset,
            vi_cursor,
            selection,
            placements,
            palette: _,
            hyperlinks: _,
        } = frame;
        self.cols != size.cols
            || self.rows != size.rows
            || self.cursor != Some(*cursor)
            || self.display_offset != display_offset.0
            || self.vi_cursor != *vi_cursor
            || self.selection != *selection
            || placements
                .as_ref()
                .is_some_and(|placements| *placements != self.placements)
    }

    /// Applies `frame`'s viewport sections to this view.
    ///
    /// The `None` sections are left alone, and `suppress_cursor` is
    /// never written.
    ///
    /// # Invariants
    ///
    /// It mutates the view exactly when [`Self::differs_from`] reports
    /// `true`.
    pub fn apply(&mut self, frame: &Frame) {
        let Frame {
            size,
            rows: _,
            cursor,
            display_offset,
            vi_cursor,
            selection,
            placements,
            palette: _,
            hyperlinks: _,
        } = frame;
        self.cols = size.cols;
        self.rows = size.rows;
        self.cursor = Some(*cursor);
        self.display_offset = display_offset.0;
        self.vi_cursor = *vi_cursor;
        self.selection = *selection;
        if let Some(placements) = placements {
            self.placements.clone_from(placements);
        }
    }
}

/// The painted contents of one terminal, materialized into column slots
/// from the frames applied to it.
#[derive(Component, Default)]
pub struct TerminalCells {
    /// Column slots indexed `[row][column]`.
    pub cells: Vec<Vec<GridSlot>>,
    /// OSC 8 hyperlinks indexed by id. Every applied frame merges its
    /// `hyperlinks` into this table, and a known id is never
    /// overwritten.
    pub hyperlinks: Vec<(HyperlinkId, HyperlinkUri)>,
    /// The live palette set by the last frame that carried one;
    /// symbolic cell colors resolve against it.
    pub palette: Palette,
}

impl TerminalCells {
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
        let id = cell.hyperlink?;
        Some((id, lookup_hyperlink(&self.hyperlinks, id)?))
    }

    /// Whether applying `frame` would change these cells.
    ///
    /// An absent row and a `None` section mean "unchanged", so this
    /// counts only a row inside the frame's own size, a size the rows do
    /// not hold yet, a listed palette that differs, or a hyperlink id
    /// the table lacks.
    ///
    /// # Invariants
    ///
    /// Returns `true` exactly when [`Self::apply`] mutates something.
    pub fn differs_from(&self, frame: &Frame) -> bool {
        let Frame {
            size,
            rows,
            cursor: _,
            display_offset: _,
            vi_cursor: _,
            selection: _,
            placements: _,
            palette,
            hyperlinks,
        } = frame;
        self.size_differs(size.cols, size.rows)
            || rows.iter().any(|row| row.line.0 < size.rows)
            || palette
                .as_ref()
                .is_some_and(|palette| *palette != self.palette)
            || hyperlinks.iter().any(|link| !self.knows_hyperlink(link.id))
    }

    /// Applies `frame`'s content sections to these cells.
    ///
    /// Every row the frame carries inside its size replaces the slots at
    /// that line, resolving its hyperlink ids against the table, into
    /// which this frame's own definitions are merged first; the `None`
    /// sections are left alone.
    ///
    /// # Invariants
    ///
    /// After `apply` returns there are exactly `frame.size.rows` rows,
    /// each holding exactly `frame.size.cols` slots. It mutates the
    /// cells exactly when [`Self::differs_from`] reports `true`.
    pub fn apply(&mut self, frame: &Frame) {
        let Frame {
            size,
            rows,
            cursor: _,
            display_offset: _,
            vi_cursor: _,
            selection: _,
            placements: _,
            palette,
            hyperlinks,
        } = frame;
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
        if let Some(palette) = palette {
            self.palette.clone_from(palette);
        }
    }

    fn size_differs(&self, cols: u16, rows: u16) -> bool {
        self.cells.len() != usize::from(rows)
            || self.cells.iter().any(|row| row.len() != usize::from(cols))
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
        let hyperlink = run
            .hyperlink_id
            .filter(|id| lookup_hyperlink(hyperlinks, *id).is_some());
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
                fg: run.fg,
                bg: run.bg,
                style: run.style.bits(),
                hyperlink,
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
impl TerminalView {
    /// A one-by-one view that already mirrors [`quiet_frame`].
    pub(crate) fn settled() -> Self {
        Self {
            cols: 1,
            rows: 1,
            cursor: Some(Cursor::default()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
impl TerminalCells {
    /// A one-by-one cell grid that already mirrors [`quiet_frame`].
    pub(crate) fn settled() -> Self {
        Self {
            cells: vec![vec![GridSlot::Empty]],
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
        PlacementSize, Rgb, Row, SelectionGeometry, Style,
    };
    use orzma_vt::prelude::{DirtyRow, ViewportLine};

    fn cell_with_link(text: &str, link: Option<u32>) -> GridCell {
        GridCell {
            text: text.to_string(),
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: 0,
            hyperlink: link.map(HyperlinkId),
        }
    }

    /// A retained table holding the single entry a linked-cell fixture
    /// needs, since a cell stores only the id.
    fn link_table(id: u32, uri: &str) -> Vec<(HyperlinkId, HyperlinkUri)> {
        vec![(HyperlinkId(id), HyperlinkUri::new(uri))]
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

    fn dirty_row(line: u16, text: &str) -> DirtyRow {
        DirtyRow {
            line: ViewportLine(line),
            contents: Row::from(vec![run_with_link(text, None)]),
        }
    }

    /// Asserts that a visible cursor reports its viewport cell and a
    /// packed style with the visible bit set.
    ///
    /// Case: the shell sits at an ordinary prompt with the caret
    /// shown.
    #[test]
    fn current_cursor_pos_and_style_returns_packed_style_when_not_suppressed() {
        let view = TerminalView {
            rows: 24,
            cursor: Some(visible_block_cursor()),
            suppress_cursor: false,
            ..Default::default()
        };
        let (pos, style) = view.current_cursor_pos_and_style();
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
        let mut view = TerminalView {
            cols: 80,
            rows: 24,
            ..TerminalView::default()
        };
        view.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(23),
                column: GridColumn(5),
            },
            ..Cursor::default()
        });
        assert_eq!(view.cursor_viewport_cell(), Some((5, 23)));

        view.display_offset = 3;
        assert_eq!(view.cursor_viewport_cell(), None);

        view.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(10),
                column: GridColumn(0),
            },
            ..Cursor::default()
        });
        assert_eq!(view.cursor_viewport_cell(), Some((0, 13)));
    }

    /// Asserts that the `_or_top` fallback keeps the cursor's column on row
    /// `0` when the line is scrolled out of view, and yields the origin when
    /// no cursor exists.
    ///
    /// Case: the user scrolls back through history during an IME
    /// composition, pushing the prompt's cursor row out of the viewport.
    #[test]
    fn cursor_viewport_cell_or_top_keeps_the_column_when_off_viewport() {
        let mut view = TerminalView {
            cols: 80,
            rows: 24,
            ..TerminalView::default()
        };
        assert_eq!(view.cursor_viewport_cell_or_top(), (0, 0));

        view.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(23),
                column: GridColumn(5),
            },
            ..Cursor::default()
        });
        view.display_offset = 3;
        assert_eq!(view.cursor_viewport_cell_or_top(), (5, 0));
    }

    /// Asserts that suppression clears the visible bit of the packed
    /// style.
    ///
    /// Case: an IME composition temporarily hides the caret.
    #[test]
    fn current_cursor_pos_and_style_clears_visible_bit_when_suppressed() {
        let view = TerminalView {
            rows: 24,
            cursor: Some(visible_block_cursor()),
            suppress_cursor: true,
            ..Default::default()
        };
        let (_pos, style) = view.current_cursor_pos_and_style();
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    /// Asserts that suppression clears only the visible bit while the
    /// vi cursor's projected position is still reported.
    ///
    /// Case: an IME composition hides the caret while a projected
    /// cursor position is already recorded on the mirror.
    #[test]
    fn suppress_cursor_does_not_affect_vi_cursor_position() {
        let view = TerminalView {
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
        let (pos, style) = view.current_cursor_pos_and_style();
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
        let view = TerminalView {
            rows: 24,
            display_offset: 30,
            cursor: Some(visible_block_cursor()),
            ..Default::default()
        };
        let (pos, style) = view.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::ZERO);
        assert_eq!(style, 0);
    }

    /// Asserts that a moved cursor is a difference, and that applying
    /// the frame settles the view so the cursor round-trips to a
    /// matching state.
    ///
    /// Case: the user presses an arrow key and the application moves
    /// the caret without repainting a cell.
    #[test]
    fn a_moved_cursor_differs() {
        let mut view = TerminalView::settled();
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
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert!(!view.differs_from(&frame));
    }

    /// Asserts that a moved viewport (display offset) is a difference,
    /// and that applying the frame settles the view so the offset
    /// round-trips to a matching state.
    ///
    /// Case: a frame that only moves the display offset reaches a settled
    /// mirror.
    #[test]
    fn a_moved_viewport_differs() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            display_offset: DisplayOffset(7),
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert!(!view.differs_from(&frame));
    }

    /// Asserts that a placements list replaces the mirror wholesale,
    /// including down to empty, and that `None` leaves it alone.
    ///
    /// Case: a program mounts a webview, a quiet frame follows, and the
    /// program then unmounts it, so the next frame carries an empty
    /// list.
    #[test]
    fn placements_replace_wholesale_and_none_keeps_them() {
        let mut view = TerminalView::settled();
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
        assert!(view.differs_from(&mounted));
        view.apply(&mounted);
        assert_eq!(view.placements, vec![placed]);

        let unchanged = quiet_frame();
        assert!(!view.differs_from(&unchanged));
        view.apply(&unchanged);
        assert_eq!(view.placements, vec![placed]);

        let hidden = Frame {
            placements: Some(vec![]),
            ..quiet_frame()
        };
        assert!(view.differs_from(&hidden));
        view.apply(&hidden);
        assert_eq!(view.placements, vec![]);
    }

    /// Asserts that a frame the view reports no difference for leaves
    /// every view field untouched.
    ///
    /// Case: a frame repeats the viewport state the view already holds,
    /// with the cursor away from the origin, a vi cursor active, a
    /// selection in progress, and the viewport scrolled into history.
    #[test]
    fn a_view_that_reports_no_difference_is_not_mutated_by_apply() {
        let cursor = Cursor {
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            ..Cursor::default()
        };
        let vi_cursor = ViCursor {
            point: GridPoint {
                line: GridLine(4),
                column: GridColumn(5),
            },
        };
        let selection = SelectionRange {
            start: GridPoint {
                line: GridLine(0),
                column: GridColumn(0),
            },
            end: GridPoint {
                line: GridLine(1),
                column: GridColumn(2),
            },
            geometry: SelectionGeometry::Linear,
        };
        let placed = AnchoredPlacement {
            id: InstanceId(9),
            point: GridPoint {
                line: GridLine(1),
                column: GridColumn(1),
            },
            size: PlacementSize { rows: 1, cols: 1 },
        };
        let mut view = TerminalView {
            cols: 7,
            rows: 6,
            cursor: Some(cursor),
            vi_cursor: Some(vi_cursor),
            selection: Some(selection),
            display_offset: 4,
            placements: vec![placed],
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 7, rows: 6 },
            cursor,
            display_offset: DisplayOffset(4),
            vi_cursor: Some(vi_cursor),
            selection: Some(selection),
            placements: Some(vec![placed]),
            ..quiet_frame()
        };
        assert!(!view.differs_from(&frame));
        let before = (
            view.cols,
            view.rows,
            view.cursor,
            view.display_offset,
            view.vi_cursor,
            view.selection,
            view.placements.clone(),
        );
        view.apply(&frame);
        assert_eq!(
            (
                view.cols,
                view.rows,
                view.cursor,
                view.display_offset,
                view.vi_cursor,
                view.selection,
                view.placements.clone(),
            ),
            before
        );
    }

    /// Asserts that a lookup outside the populated grid returns `None`.
    ///
    /// Case: the pointer hovers over the padding beyond the last row
    /// or column while the grid is smaller than the window.
    #[test]
    fn hyperlink_at_returns_none_when_out_of_bounds() {
        let cells = TerminalCells {
            cells: vec![vec![], vec![]],
            ..Default::default()
        };
        assert!(cells.hyperlink_at(99, 0).is_none());
        assert!(cells.hyperlink_at(0, 99).is_none());
    }

    /// Asserts that a column no run covered resolves to no hyperlink.
    ///
    /// Case: the pointer hovers a column of the row that no attribute
    /// run painted this frame.
    #[test]
    fn hyperlink_at_returns_none_for_empty_slot() {
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Empty; 4]],
            ..Default::default()
        };
        assert!(cells.hyperlink_at(0, 0).is_none());
    }

    /// Asserts that a linked cell resolves to its hyperlink id and URI.
    ///
    /// Case: the user hovers an OSC 8 link a shell printed, and the
    /// input layer asks which link sits under the pointer.
    #[test]
    fn hyperlink_at_returns_id_and_uri_for_linked_cell() {
        let cell = cell_with_link("x", Some(7));
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(cell)]],
            hyperlinks: link_table(7, "https://example"),
            ..Default::default()
        };
        let (id, uri) = cells.hyperlink_at(0, 0).expect("hyperlink present");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
    }

    /// Asserts that an unlinked cell resolves to `None`.
    ///
    /// Case: the user hovers plain shell output that carries no
    /// hyperlink.
    #[test]
    fn hyperlink_at_returns_none_for_unlinked_cell() {
        let cell = cell_with_link("x", None);
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(cell)]],
            ..Default::default()
        };
        assert!(cells.hyperlink_at(0, 0).is_none());
    }

    /// Asserts that both columns of a wide grapheme resolve to the
    /// same hyperlink while the cell after it stays unlinked.
    ///
    /// Case: a CJK character inside an OSC 8 link spans two columns,
    /// and the user may hover either half.
    #[test]
    fn hyperlink_at_resolves_both_halves_of_wide_char() {
        let wide_linked = cell_with_link("あ", Some(7));
        let trailing = cell_with_link("b", None);
        let cells = TerminalCells {
            cells: vec![vec![
                GridSlot::Cell(wide_linked),
                GridSlot::WideTrailer,
                GridSlot::Cell(trailing),
            ]],
            hyperlinks: link_table(7, "https://example"),
            ..Default::default()
        };
        let (id, uri) = cells.hyperlink_at(0, 0).expect("left half should resolve");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
        let (id, uri) = cells.hyperlink_at(0, 1).expect("right half should resolve");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
        assert!(cells.hyperlink_at(0, 2).is_none());
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
        let table = link_table(7, "https://example");
        let slots = runs_to_cells(&runs, 2, &table);
        assert_eq!(
            slots[0].cell().and_then(|c| c.hyperlink),
            Some(HyperlinkId(7))
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
        assert_eq!(slots[1], GridSlot::WideTrailer);
        assert_eq!(slots[2].cell().map(|c| c.text.as_str()), Some("z"));
        assert_eq!(slots[3], GridSlot::Empty);
    }

    /// Asserts that a wide grapheme landing on the grid's last column
    /// produces one cell slot and no trailing `WideTrailer`, since no
    /// column remains for one.
    ///
    /// Case: a CJK character is the only character a single-column-wide
    /// pane can hold.
    #[test]
    fn runs_to_cells_stops_a_wide_grapheme_at_the_last_column() {
        let slots = runs_to_cells(&[run_with_link("あ", None)], 1, &[]);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].cell().map(|c| c.text.as_str()), Some("あ"));
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

    /// Asserts that a carried row replaces that row's slots when
    /// applied, while a row the frame does not touch is leveled to the
    /// full row width instead of keeping its original length.
    ///
    /// Case: a build prints one line of a two-row pane.
    #[test]
    fn a_carried_row_replaces_its_slots_and_leaves_other_rows_full_width() {
        let mut cells = TerminalCells {
            cells: vec![vec![], vec![]],
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(1, "x")],
            ..quiet_frame()
        };
        assert!(cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells[1][0].cell().map(|c| c.text.as_str()), Some("x"));
        assert_eq!(cells.cells[0], vec![GridSlot::Empty; 2]);
    }

    /// Asserts that cells built at the frame's size but without cell
    /// rows still differ, and that applying the frame gives every row
    /// a slot and fills the carried ones.
    ///
    /// Case: the host pre-sizes the grid to the PTY geometry before the
    /// VT's bootstrap repaint arrives at that same size.
    #[test]
    fn a_pre_sized_grid_without_cells_takes_the_frame_rows() {
        let mut cells = TerminalCells {
            cells: vec![vec![], vec![]],
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
            ..quiet_frame()
        };
        assert!(cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells.len(), 2);
        assert_eq!(cells.cells[1][0].cell().map(|c| c.text.as_str()), Some("b"));
        assert!(!cells.differs_from(&Frame {
            size: GridSize { cols: 2, rows: 2 },
            ..quiet_frame()
        }));
    }

    /// Asserts that a frame with fewer rows than the cells truncates the
    /// cell rows to the new height.
    ///
    /// Case: the user drags the window shorter and the VT's repaint at
    /// the new height arrives.
    #[test]
    fn a_shrinking_size_truncates_the_cells() {
        let mut cells = TerminalCells {
            cells: vec![vec![], vec![], vec![]],
            ..Default::default()
        };
        let frame = Frame {
            rows: vec![dirty_row(0, "a")],
            ..quiet_frame()
        };
        assert!(cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells.len(), 1);
        assert_eq!(cells.cells[0][0].cell().map(|c| c.text.as_str()), Some("a"));
    }

    /// Asserts that a frame changing only the column count is a
    /// difference and repaints the rows it carries at the new width.
    ///
    /// Case: the user drags the window wider without changing its
    /// height, and the VT's repaint at the new width arrives.
    #[test]
    fn a_cols_only_size_change_differs_and_repaints() {
        let mut cells = TerminalCells::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 1 },
            rows: vec![dirty_row(0, "abc")],
            ..quiet_frame()
        };
        assert!(cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells[0].len(), 3);
        assert!(!cells.differs_from(&Frame {
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
        let mut cells = TerminalCells::settled();
        let frame = Frame {
            rows: vec![dirty_row(5, "x")],
            ..quiet_frame()
        };
        assert!(!cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells.len(), 1);
    }

    /// Asserts that a size change resizes the cell rows before the
    /// frame's rows are applied.
    ///
    /// Case: the user drags the window taller and the VT's next frame
    /// carries every row of the new size.
    #[test]
    fn a_size_change_resizes_the_cells() {
        let mut cells = TerminalCells::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
            ..quiet_frame()
        };
        assert!(cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells.len(), 2);
        assert_eq!(cells.cells[1][0].cell().map(|c| c.text.as_str()), Some("b"));
    }

    /// Asserts that a column-count change levels every retained row to
    /// the new width, including the rows the frame does not carry.
    ///
    /// Case: the user drags the window wider and the repaint that
    /// follows names only the row the cursor sits on.
    #[test]
    fn a_cols_change_levels_every_row_not_only_the_carried_ones() {
        let mut cells = TerminalCells::default();
        cells.apply(&Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(0, "ab"), dirty_row(1, "cd")],
            ..quiet_frame()
        });
        cells.apply(&Frame {
            size: GridSize { cols: 5, rows: 2 },
            rows: vec![dirty_row(0, "abcde")],
            ..quiet_frame()
        });
        assert_eq!(cells.cells.len(), 2);
        assert!(
            cells.cells.iter().all(|row| row.len() == 5),
            "every row is as wide as the grid"
        );
    }

    /// Asserts that a palette replaces the mirror and `None` keeps it.
    ///
    /// Case: one frame recolors the background once, and every later
    /// frame carries no palette.
    #[test]
    fn a_palette_replaces_the_mirror_and_none_keeps_it() {
        let mut cells = TerminalCells::settled();
        let palette = Palette {
            background: Rgb { r: 9, g: 8, b: 7 },
            ..Palette::default()
        };
        let recolored = Frame {
            palette: Some(palette),
            ..quiet_frame()
        };
        assert!(cells.differs_from(&recolored));
        cells.apply(&recolored);
        assert_eq!(cells.palette.background, Rgb { r: 9, g: 8, b: 7 });
        assert!(!cells.differs_from(&quiet_frame()));
    }

    /// Asserts that hyperlinks merge without overwriting a known id,
    /// and that a known id alone is not a difference.
    ///
    /// Case: a later frame re-sends a definition the mirror already
    /// holds alongside a genuinely new one.
    #[test]
    fn hyperlinks_merge_without_overwrite() {
        let mut cells = TerminalCells {
            hyperlinks: vec![(HyperlinkId(1), HyperlinkUri::new("https://old"))],
            ..TerminalCells::settled()
        };
        let repeated = Frame {
            hyperlinks: vec![Hyperlink {
                id: HyperlinkId(1),
                uri: HyperlinkUri::new("https://CHANGED"),
            }],
            ..quiet_frame()
        };
        assert!(!cells.differs_from(&repeated));

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
        assert!(cells.differs_from(&extended));
        cells.apply(&extended);
        assert_eq!(cells.hyperlinks.len(), 2);
        assert_eq!(cells.hyperlinks[0].1.as_str(), "https://old");
        assert_eq!(cells.hyperlinks[1].1.as_str(), "https://new");
    }

    /// Asserts that a row resolves a hyperlink id defined by an earlier
    /// frame's table.
    ///
    /// Case: a link's definition arrived in one frame and the row that
    /// references it is repainted in a later one.
    #[test]
    fn a_row_resolves_a_hyperlink_from_an_earlier_frame() {
        let mut cells = TerminalCells::settled();
        cells.apply(&Frame {
            hyperlinks: vec![Hyperlink {
                id: HyperlinkId(4),
                uri: HyperlinkUri::new("https://earlier"),
            }],
            ..quiet_frame()
        });
        cells.apply(&Frame {
            rows: vec![DirtyRow {
                line: ViewportLine(0),
                contents: Row::from(vec![run_with_link("a", Some(HyperlinkId(4)))]),
            }],
            ..quiet_frame()
        });
        assert_eq!(
            cells.hyperlink_at(0, 0).map(|(_, uri)| uri.as_str()),
            Some("https://earlier")
        );
    }

    /// Asserts that a frame carrying nothing new reports no difference
    /// for either component.
    ///
    /// Case: a frame repeats what the mirror already holds.
    #[test]
    fn a_quiet_frame_differs_from_neither_component() {
        assert!(!TerminalView::settled().differs_from(&quiet_frame()));
        assert!(!TerminalCells::settled().differs_from(&quiet_frame()));
    }

    /// Asserts that a size change alone is a difference for the view,
    /// and that applying the frame updates its column and row counts
    /// even though the frame carries no rows of its own.
    ///
    /// Case: a frame that only changes the size reaches a settled view.
    #[test]
    fn a_new_size_alone_differs() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert_eq!((view.cols, view.rows), (3, 2));
        assert!(!view.differs_from(&frame));
    }

    /// Asserts that a size change alone is a difference for the cells,
    /// and that applying the frame resizes the cell rows to match even
    /// though the frame carries no rows of its own.
    ///
    /// Case: a frame that only changes the size reaches settled cells.
    #[test]
    fn a_new_size_alone_resizes_the_cell_rows() {
        let mut cells = TerminalCells::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            ..quiet_frame()
        };
        assert!(cells.differs_from(&frame));
        cells.apply(&frame);
        assert_eq!(cells.cells.len(), 2);
        assert!(!cells.differs_from(&frame));
    }

    /// Asserts that a shrinking size change is a difference for the
    /// view, and that applying the frame updates its column and row
    /// counts to the smaller size.
    ///
    /// Case: the user drags the window shorter and the VT's repaint at
    /// the new height arrives.
    #[test]
    fn a_shrinking_size_updates_the_view_dims() {
        let mut view = TerminalView {
            cols: 1,
            rows: 3,
            ..TerminalView::settled()
        };
        let frame = Frame {
            rows: vec![dirty_row(0, "a")],
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert_eq!((view.cols, view.rows), (1, 1));
    }

    /// Asserts that a frame changing only the column count is a
    /// difference for the view, and that applying it updates the
    /// column count while leaving the row count alone.
    ///
    /// Case: the user drags the window wider without changing its
    /// height, and the VT's repaint at the new width arrives.
    #[test]
    fn a_cols_only_size_change_differs_and_updates_the_view() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 1 },
            rows: vec![dirty_row(0, "abc")],
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert_eq!((view.cols, view.rows), (3, 1));
        assert!(!view.differs_from(&Frame {
            size: GridSize { cols: 3, rows: 1 },
            ..quiet_frame()
        }));
    }

    /// Asserts that a frame the cells report no difference for leaves
    /// the cells, hyperlinks and palette untouched.
    ///
    /// Case: a frame repeats the content state the cells already hold,
    /// with a linked cell, its hyperlink table entry, and a
    /// non-default palette already in place.
    #[test]
    fn a_cells_that_reports_no_difference_is_not_mutated_by_apply() {
        let linked = cell_with_link("x", Some(7));
        let mut cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(linked)]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example"))],
            palette: Palette {
                background: Rgb { r: 9, g: 8, b: 7 },
                ..Palette::default()
            },
        };
        let frame = quiet_frame();
        assert!(!cells.differs_from(&frame));
        let before = (
            cells.cells.clone(),
            cells.hyperlinks.clone(),
            cells.palette.clone(),
        );
        cells.apply(&frame);
        assert_eq!(
            (
                cells.cells.clone(),
                cells.hyperlinks.clone(),
                cells.palette.clone(),
            ),
            before
        );
    }
}
