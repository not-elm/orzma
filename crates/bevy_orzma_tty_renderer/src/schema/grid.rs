//! The renderer's CPU-side mirror of one terminal, materialized into
//! cells from the frames applied to it.

use crate::schema::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, GridColumn,
    GridLine, GridPoint, Hyperlink, HyperlinkId, HyperlinkUri, Palette, Run, SelectionRange,
    ViCursor,
};
use bevy::prelude::*;
use orzma_vt::prelude::Frame;
#[cfg(test)]
use orzma_vt::prelude::GridSize;

/// One materialized cell of the renderer's CPU-side grid, expanded
/// from the frame's [`crate::schema::Run`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    /// The cell's glyph followed by the marks combined onto it.
    pub text: String,
    /// Display width in columns: 2 for a wide glyph, 1 otherwise. A cell
    /// built outside [`TerminalGrid::apply`] may carry 0; such a cell
    /// paints no glyph and takes no column.
    pub width: u8,
    /// Active-grid coordinates of the cell.
    pub point: GridPoint,
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
    /// Whether this cell paints no glyph: a zero-width cell, or one whose
    /// text is empty or all whitespace.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.width == 0 || self.text.trim().is_empty()
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
    /// Cell grid indexed `[row][cell_index]`.
    pub cells: Vec<Vec<GridCell>>,
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
    /// any. `col` is a column coordinate, not a cell index — a wide
    /// cell (width 2) matches both of its columns, and a width-0 cell
    /// matches no column. Returns `None` for out-of-bounds or unlinked
    /// cells.
    //
    // NOTE: `self.cells[row]` is cell-indexed (one entry per glyph from
    //       `runs_to_cells`), so a column-to-cell walk is required —
    //       direct `cells[row][col]` indexing would desynchronize after
    //       any wide char. Must mirror the column-advance logic in
    //       `material::fill_cells`.
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
        self.cells.resize_with(usize::from(size.rows), Vec::new);
        for link in hyperlinks {
            if !self.knows_hyperlink(link.id) {
                self.hyperlinks.push((link.id, link.uri.clone()));
            }
        }
        for row in rows {
            let Some(slot) = self.cells.get_mut(usize::from(row.line.0)) else {
                continue;
            };
            let line = row.line.to_grid(*display_offset);
            *slot = runs_to_cells(&row.contents, line, &self.hyperlinks);
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

/// Materializes one row's attribute runs into cells, resolving each
/// run's hyperlink id against the retained table.
///
/// Column advance follows each run's widths: a `char` at width two takes
/// two columns, and a `char` at width zero joins the cell before it.
fn runs_to_cells(
    runs: &[Run],
    line: GridLine,
    hyperlinks: &[(HyperlinkId, HyperlinkUri)],
) -> Vec<GridCell> {
    // NOTE: The column walk here must advance exactly as
    // `material::fill_cells` re-derives it from `GridCell::width`;
    // a change to one without the other misaligns every glyph after
    // the first wide character.
    let mut out: Vec<GridCell> =
        Vec::with_capacity(runs.iter().map(|run| usize::from(run.cols)).sum());
    let mut column: u16 = 0;
    for run in runs {
        debug_assert!(
            run.widths.is_empty() || run.widths.len() == run.text.chars().count(),
            "a run's widths cover each char of its text"
        );
        debug_assert!(
            run.widths.is_empty()
                || run.widths.iter().map(|w| u32::from(*w)).sum::<u32>() == u32::from(run.cols),
            "a run's widths sum to its columns"
        );
        debug_assert!(
            run.widths.iter().all(|w| *w <= 2) && run.widths.first() != Some(&0),
            "a run's widths are 0, 1 or 2 and never start with a continuation"
        );
        let hyperlink = run.hyperlink_id.and_then(|id| {
            lookup_hyperlink(hyperlinks, id).map(|uri| Hyperlink {
                id,
                uri: uri.clone(),
            })
        });
        let mut widths = run.widths.iter().copied();
        for c in run.text.chars() {
            let width = widths.next().unwrap_or(1);
            if width == 0
                && let Some(base) = out.last_mut()
            {
                base.text.push(c);
                continue;
            }
            out.push(GridCell {
                text: c.to_string(),
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
impl TerminalGrid {
    /// A one-by-one grid that already mirrors [`quiet_frame`].
    pub(crate) fn settled() -> Self {
        Self {
            cols: 1,
            rows: 1,
            cells: vec![vec![]],
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
            widths: Vec::new(),
            hyperlink_id,
        }
    }

    fn run_with_widths(text: &str, widths: &[u8]) -> Run {
        let mut run = run_with_link(text, None);
        run.cols = if widths.is_empty() {
            text.chars().count() as u16
        } else {
            widths.iter().map(|w| u16::from(*w)).sum()
        };
        run.widths = widths.to_vec();
        run
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
    /// driven by the run's widths.
    ///
    /// Case: a row mixes a wide CJK glyph with ASCII text on a
    /// scrolled-back history line.
    #[test]
    fn runs_to_cells_assigns_points_by_run_widths() {
        let cells = runs_to_cells(&[run_with_widths("あb", &[2, 1])], GridLine(-3), &[]);
        assert_eq!(cells[0].width, 2);
        assert_eq!(
            cells[0].point,
            GridPoint {
                line: GridLine(-3),
                column: GridColumn(0),
            }
        );
        assert_eq!(cells[1].width, 1);
        assert_eq!(
            cells[1].point,
            GridPoint {
                line: GridLine(-3),
                column: GridColumn(2),
            }
        );
    }

    /// Asserts that a run without widths yields one one-column cell per
    /// `char`, whatever the characters are.
    ///
    /// Case: a frame carries an ASCII row on the empty-width path.
    #[test]
    fn runs_to_cells_treats_an_empty_width_list_as_one_column_each() {
        let cells = runs_to_cells(&[run_with_widths("ab", &[])], GridLine(0), &[]);
        assert_eq!(cells.len(), 2);
        assert!(cells.iter().all(|cell| cell.width == 1));
        assert_eq!(cells[1].point.column, GridColumn(1));
    }

    /// Asserts that a zero-width `char` joins the text of the cell before
    /// it instead of becoming a cell of its own.
    ///
    /// Case: a frame carries `e` followed by a combining acute accent.
    #[test]
    fn runs_to_cells_joins_a_zero_width_char_to_the_previous_cell() {
        let cells = runs_to_cells(
            &[run_with_widths("e\u{0301}x", &[1, 0, 1])],
            GridLine(0),
            &[],
        );
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].text, "e\u{0301}");
        assert_eq!(cells[0].width, 1);
        assert_eq!(cells[1].text, "x");
        assert_eq!(cells[1].point.column, GridColumn(1));
    }

    /// Asserts that a run's `char`s are never re-measured: a wide glyph
    /// declared at width one takes one column.
    ///
    /// Case: a frame declares a CJK glyph at width one on a row the VT already
    /// laid out.
    #[test]
    fn runs_to_cells_does_not_remeasure_the_text() {
        let cells = runs_to_cells(&[run_with_widths("あb", &[1, 1])], GridLine(0), &[]);
        assert_eq!(cells[0].width, 1);
        assert_eq!(cells[1].point.column, GridColumn(1));
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
        assert_eq!(grid.cells[1][0].text, "b");
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
        assert_eq!(grid.cells[0][0].text, "a");
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
        assert_eq!(grid.cells[1][0].text, "b");
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
            grid.cells[0][0].hyperlink.as_ref().map(|h| h.uri.as_str()),
            Some("https://earlier")
        );
    }
}
