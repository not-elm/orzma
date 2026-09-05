//! Selection vocabulary — the span a selection covers, how it is shaped,
//! and which side of a cell an endpoint sits on — plus the selection one
//! screen owns and its projection into that vocabulary.

use crate::screen::grid::LineId;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};

/// A renderable selection: normalized active-grid endpoints plus the
/// shape they span.
///
/// `start` is the top-left and `end` the bottom-right of the selected
/// cells, both inclusive — anchor/moving-end order is already resolved
/// and cell-side trimming applied by the VT. The endpoints are raw
/// grid positions and do not move when the user scrolls; project them
/// with [`crate::prelude::GridLine::to_viewport`] to place the
/// highlight on screen.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SelectionRange {
    /// Top-left selected cell (inclusive).
    pub start: GridPoint,
    /// Bottom-right selected cell (inclusive).
    pub end: GridPoint,
    /// The shape spanned between the endpoints.
    pub geometry: SelectionGeometry,
}

/// The shape a [`SelectionRange`] spans between its endpoints.
///
/// Wider than [`SelectionKind`]: the two current selection kinds only
/// ever convert to `Linear` or `Lines`. `Block` exists for the
/// renderer to support once a future selection kind needs it, but the
/// conversion does not produce it yet.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SelectionGeometry {
    /// A cell run wrapping at the end of each row.
    Linear,
    /// A rectangular column block.
    Block,
    /// Whole rows.
    Lines,
}

impl From<SelectionKind> for SelectionGeometry {
    fn from(value: SelectionKind) -> Self {
        match value {
            SelectionKind::Lines => Self::Lines,
            _ => Self::Linear,
        }
    }
}

/// Selection granularity.
// TODO: Add the `Block` (rectangular column) and `Semantic` (snapped to
// word boundaries) kinds once vi mode and semantic selection land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell-by-cell, wrapping at the end of each line.
    Simple,
    /// Whole lines.
    Lines,
}

/// Which half of a cell a selection endpoint sits in.
///
/// Decides whether the cell under the cursor is included: an endpoint on the
/// far side of a cell takes that cell, an endpoint on the near side stops
/// before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellSide {
    /// Left half.
    Left,
    /// Right half.
    Right,
}

/// The selection one screen owns: two endpoints on that screen's rows
/// plus the granularity between them.
///
/// Endpoints are stored as a row identity and a cell boundary, so the
/// selection follows its rows when output scrolls them into history and
/// does not depend on the projection the frame carries.
#[derive(Debug)]
pub(crate) struct ScreenSelection {
    state: Option<SelectionState>,
}

/// One endpoint of a screen selection.
///
/// `boundary` is a cell boundary in `0..=cols`: the left side of column
/// `c` is boundary `c`, its right side is `c + 1`, so `cols` is the
/// right edge of the row. Two `(column, side)` pairs that name the same
/// boundary compare equal here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionEnd {
    pub line: LineId,
    pub boundary: u16,
}

/// What a selection resolves to at one instant.
///
/// `None` and `Empty` both project nothing; they differ in whether any
/// state exists, which `clear` reports and the projection does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// No selection, or an endpoint whose row has left the ring.
    None,
    /// A selection whose two ends enclose no cell.
    Empty,
    /// The cells the selection covers, normalized for the frame.
    Range(SelectionRange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionState {
    anchor: SelectionEnd,
    moving: SelectionEnd,
    kind: SelectionKind,
}

impl SelectionEnd {
    /// The boundary a host-reported cell half stands for.
    pub fn at(line: LineId, column: GridColumn, side: CellSide) -> Self {
        let boundary = match side {
            CellSide::Left => column.0,
            CellSide::Right => column.0 + 1,
        };
        Self { line, boundary }
    }
}

impl ScreenSelection {
    /// Builds a screen with nothing selected.
    pub fn new() -> Self {
        Self { state: None }
    }

    /// Anchors a new selection with both ends on `end`, replacing any
    /// active one; returns whether the state changed.
    pub fn start(&mut self, end: SelectionEnd, kind: SelectionKind) -> bool {
        let next = SelectionState {
            anchor: end,
            moving: end,
            kind,
        };
        if self.state == Some(next) {
            return false;
        }
        self.state = Some(next);
        true
    }

    /// Moves the active selection's moving end; returns whether it
    /// moved. A no-op without an active selection.
    pub fn extend(&mut self, end: SelectionEnd) -> bool {
        let Some(state) = &mut self.state else {
            return false;
        };
        if state.moving == end {
            return false;
        }
        state.moving = end;
        true
    }

    /// Drops the selection; returns whether there was one.
    pub fn clear(&mut self) -> bool {
        self.state.take().is_some()
    }

    /// Resolves the endpoints through `line_of` into the range a frame
    /// carries.
    ///
    /// Boundaries past `cols` are clamped to it. A Simple selection is
    /// normalized by `(line, boundary)`, then each boundary becomes an
    /// inclusive cell: a start on a row's right edge moves to the next
    /// row's first cell and an end on a row's left edge moves to the
    /// previous row's last cell; ends that cross after that enclose no
    /// cell. A Lines selection spans whole rows between the two
    /// endpoints' rows and never wraps.
    ///
    /// # Invariants
    ///
    /// Endpoints are ordered only after resolving to [`GridLine`];
    /// nothing orders the ring by [`LineId`].
    pub fn resolve(
        &self,
        mut line_of: impl FnMut(LineId) -> Option<GridLine>,
        cols: u16,
    ) -> Resolved {
        debug_assert!(cols > 0, "a screen never has zero columns");
        let Some(state) = self.state else {
            return Resolved::None;
        };
        let (Some(anchor_line), Some(moving_line)) =
            (line_of(state.anchor.line), line_of(state.moving.line))
        else {
            return Resolved::None;
        };
        let anchor = (anchor_line.0, state.anchor.boundary.min(cols));
        let moving = (moving_line.0, state.moving.boundary.min(cols));
        let (start, end) = if anchor <= moving {
            (anchor, moving)
        } else {
            (moving, anchor)
        };
        let last_column = cols - 1;
        let (start, end) = match state.kind {
            SelectionKind::Lines => ((start.0, 0), (end.0, last_column)),
            SelectionKind::Simple => {
                if start == end {
                    return Resolved::Empty;
                }
                let start = if start.1 == cols {
                    (start.0 + 1, 0)
                } else {
                    start
                };
                let end = if end.1 == 0 {
                    (end.0 - 1, last_column)
                } else {
                    (end.0, end.1 - 1)
                };
                if start > end {
                    return Resolved::Empty;
                }
                (start, end)
            }
        };
        Resolved::Range(SelectionRange {
            start: GridPoint {
                line: GridLine(start.0),
                column: GridColumn(start.1),
            },
            end: GridPoint {
                line: GridLine(end.0),
                column: GridColumn(end.1),
            },
            geometry: SelectionGeometry::from(state.kind),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::grid::coords::ScreenLine;
    use crate::screen::grid::{Grid, GridSize, LineId};

    const COLS: u16 = 4;

    fn grid() -> Grid {
        Grid::new(
            GridSize {
                cols: COLS,
                rows: 3,
            },
            10,
        )
    }

    fn id(grid: &Grid, line: u16) -> LineId {
        grid.line_id(ScreenLine(line))
    }

    fn end(grid: &Grid, line: u16, column: u16, side: CellSide) -> SelectionEnd {
        SelectionEnd::at(id(grid, line), GridColumn(column), side)
    }

    fn point(line: i32, column: u16) -> GridPoint {
        GridPoint {
            line: GridLine(line),
            column: GridColumn(column),
        }
    }

    fn resolve(selection: &ScreenSelection, grid: &Grid) -> Resolved {
        selection.resolve(|id| grid.grid_line(id), COLS)
    }

    fn range_of(resolved: Resolved) -> SelectionRange {
        match resolved {
            Resolved::Range(range) => range,
            other => panic!("expected a range, got {other:?}"),
        }
    }

    /// Asserts that a cell's left side maps to the boundary before it
    /// and its right side to the boundary after it.
    ///
    /// Case: the host reports the half of the cell the pointer landed
    /// on, and the VT needs the cell boundary that half stands for.
    #[test]
    fn a_side_converts_to_the_adjacent_boundary() {
        let grid = grid();
        assert_eq!(end(&grid, 0, 1, CellSide::Left).boundary, 1);
        assert_eq!(end(&grid, 0, 1, CellSide::Right).boundary, 2);
    }

    /// Asserts that an empty table resolves to `Resolved::None`.
    ///
    /// Case: a frame is emitted on a terminal nothing has been selected
    /// on.
    #[test]
    fn no_state_resolves_to_none() {
        let grid = grid();
        let selection = ScreenSelection::new();
        assert!(matches!(resolve(&selection, &grid), Resolved::None));
    }

    /// Asserts that a Simple selection whose two ends share a boundary
    /// resolves to `Resolved::Empty`, and that a start which changes
    /// nothing reports `false`.
    ///
    /// Case: the user presses the mouse button and has not moved yet,
    /// then the host re-sends the same press.
    #[test]
    fn a_simple_start_alone_is_empty_and_idempotent() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        let anchor = end(&grid, 0, 1, CellSide::Left);
        assert!(selection.start(anchor, SelectionKind::Simple));
        assert!(matches!(resolve(&selection, &grid), Resolved::Empty));
        assert!(!selection.start(anchor, SelectionKind::Simple));
    }

    /// Asserts that a forward extend on one row resolves to the cells
    /// between the two boundaries, inclusive of the cell before the end
    /// boundary.
    ///
    /// Case: the user drags from the first cell across two more.
    #[test]
    fn a_forward_extend_resolves_to_the_span() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 0, 0, CellSide::Left), SelectionKind::Simple);
        assert!(selection.extend(end(&grid, 0, 2, CellSide::Right)));
        let range = range_of(resolve(&selection, &grid));
        assert_eq!(range.start, point(0, 0));
        assert_eq!(range.end, point(0, 2));
        assert_eq!(range.geometry, SelectionGeometry::Linear);
    }

    /// Asserts that an extend to the boundary the moving end already
    /// occupies reports no change, even via a different cell and side.
    ///
    /// Case: the pointer crosses from the right half of one cell into
    /// the left half of the next.
    #[test]
    fn an_extend_to_the_same_boundary_reports_no_change() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 0, 1, CellSide::Right), SelectionKind::Simple);
        assert!(!selection.extend(end(&grid, 0, 2, CellSide::Left)));
    }

    /// Asserts that a moving end above and left of the anchor is
    /// normalized so the range reads top-left to bottom-right.
    ///
    /// Case: the user drags upward from the middle of the second row.
    #[test]
    fn a_backward_drag_is_normalized() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 1, 2, CellSide::Left), SelectionKind::Simple);
        selection.extend(end(&grid, 0, 1, CellSide::Left));
        let range = range_of(resolve(&selection, &grid));
        assert_eq!(range.start, point(0, 1));
        assert_eq!(range.end, point(1, 1));
    }

    /// Asserts that a start boundary on the right edge of a row begins
    /// the range on the first cell of the next row, and an end boundary
    /// on the left edge of a row ends it on the last cell of the row
    /// above.
    ///
    /// Case: drags that begin on the right half of the last column or
    /// land on the left half of the first column.
    #[test]
    fn edge_boundaries_wrap_to_the_adjacent_row() {
        let grid = grid();
        let mut from_right_edge = ScreenSelection::new();
        from_right_edge.start(end(&grid, 0, 3, CellSide::Right), SelectionKind::Simple);
        from_right_edge.extend(end(&grid, 1, 1, CellSide::Right));
        let range = range_of(resolve(&from_right_edge, &grid));
        assert_eq!((range.start, range.end), (point(1, 0), point(1, 1)));

        let mut to_left_edge = ScreenSelection::new();
        to_left_edge.start(end(&grid, 0, 1, CellSide::Left), SelectionKind::Simple);
        to_left_edge.extend(end(&grid, 1, 0, CellSide::Left));
        let range = range_of(resolve(&to_left_edge, &grid));
        assert_eq!((range.start, range.end), (point(0, 1), point(0, 3)));
    }

    /// Asserts that a range whose wrapped ends cross resolves to
    /// `Resolved::Empty`.
    ///
    /// Case: the user presses on the right half of the last column and
    /// releases on the left half of the first column of the next row.
    #[test]
    fn a_wrap_that_crosses_itself_is_empty() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 0, 3, CellSide::Right), SelectionKind::Simple);
        selection.extend(end(&grid, 1, 0, CellSide::Left));
        assert!(matches!(resolve(&selection, &grid), Resolved::Empty));
    }

    /// Asserts that a Lines selection spans whole rows from the
    /// anchor's row to the moving end's row without any boundary wrap,
    /// so an anchor on a row's right edge still selects that row.
    ///
    /// Case: the user triple-clicks the right half of a row's last
    /// column and drags one row down.
    #[test]
    fn lines_take_whole_rows_without_wrapping() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 0, 3, CellSide::Right), SelectionKind::Lines);
        selection.extend(end(&grid, 1, 1, CellSide::Left));
        let range = range_of(resolve(&selection, &grid));
        assert_eq!((range.start, range.end), (point(0, 0), point(1, 3)));
        assert_eq!(range.geometry, SelectionGeometry::Lines);
    }

    /// Asserts that an end whose row the resolver cannot place makes
    /// the whole selection resolve to `Resolved::None`, while the state
    /// still counts as present for `clear`.
    ///
    /// Case: output scrolled the anchor's row out of the ring before the
    /// next frame.
    #[test]
    fn a_lost_row_resolves_to_none_but_keeps_the_state() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 0, 0, CellSide::Left), SelectionKind::Lines);
        assert!(matches!(selection.resolve(|_| None, COLS), Resolved::None));
        assert!(selection.clear());
        assert!(!selection.clear());
    }

    /// Asserts that a boundary past the width is clamped to the width
    /// at resolution time.
    ///
    /// Case: the grid shrank after the selection was made.
    #[test]
    fn a_boundary_past_the_width_is_clamped() {
        let grid = grid();
        let mut selection = ScreenSelection::new();
        selection.start(end(&grid, 0, 0, CellSide::Left), SelectionKind::Simple);
        selection.extend(end(&grid, 0, 3, CellSide::Right));
        let range = range_of(selection.resolve(|id| grid.grid_line(id), 2));
        assert_eq!((range.start, range.end), (point(0, 0), point(0, 1)));
    }
}
