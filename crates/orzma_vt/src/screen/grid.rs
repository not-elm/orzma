//! Cell storage: the visible screen plus the scrollback ring.

pub mod row;
pub mod run;

pub(crate) mod coords;
mod history_index;

use crate::screen::cell::Cell;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
use crate::screen::grid::history_index::HistoryIndex;
use crate::screen::grid::row::Row;
use std::collections::VecDeque;
use std::ops::{Index, IndexMut, Range};

/// Grid dimensions in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
}

/// Stable identity of one grid row, minted when the row enters the ring.
///
/// An id must be resolved against the grid that minted it: uniqueness is
/// per grid, not per terminal.
///
/// # Invariants
///
/// An id is the row's identity, not its address: it follows the row
/// wherever the row moves inside the ring. Ids are never reused within
/// one grid, so a placement anchored to one can never be re-pointed at
/// later content on the same grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineId(u64);

/// Storage-only grid: scrollback history plus the visible screen in
/// one ring.
#[derive(Debug)]
pub struct Grid {
    /// One logical ring holding history and the active screen:
    /// the last `size.rows` entries are the active screen, everything
    /// before them is history, oldest first. Index `0` is always the
    /// oldest surviving history row, and the boundary sits at
    /// `history_len`.
    rows: VecDeque<GridRow>,
    /// Active-screen dimensions; `rows` always keeps at least this
    /// many entries as its tail window.
    size: GridSize,
    /// History row cap: `history_len` never exceeds it.
    max_history: usize,
    /// The id the next row to enter the ring will carry.
    next_line_id: u64,
    /// Constant-time lookup of a history row's ring index by its id.
    history_index: HistoryIndex,
}

/// One stored row: its identity together with its cells.
#[derive(Debug)]
struct GridRow {
    id: LineId,
    cells: Row<Cell>,
}

impl Grid {
    /// Builds a grid of blank visible rows with an empty history.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        let mut rows = VecDeque::with_capacity(usize::from(size.rows));
        for id in 0..u64::from(size.rows) {
            rows.push_back(GridRow {
                id: LineId(id),
                cells: Row::filled(size.cols, Cell::default()),
            });
        }
        Self {
            rows,
            size,
            max_history,
            next_line_id: u64::from(size.rows),
            history_index: HistoryIndex::default(),
        }
    }

    /// Discards the history and rebuilds the visible rows blank.
    ///
    /// The grid keeps its size and its history cap.
    pub fn reset(&mut self) {
        self.rows.clear();
        self.history_index = HistoryIndex::default();
        for _ in 0..self.size.rows {
            self.push_blank_row();
        }
    }

    /// Grid dimensions in cells.
    pub const fn size(&self) -> GridSize {
        self.size
    }

    /// Whether every visible cell is blank and no history survives.
    ///
    /// Row ids do not count, so a grid that has scrolled and then been
    /// erased still reports blank.
    pub fn is_blank(&self) -> bool {
        self.history_len() == 0
            && self
                .rows
                .iter()
                .all(|row| row.cells.iter().all(|cell| *cell == Cell::default()))
    }

    /// Overwrites the given column range of one visible row with `fill`.
    pub fn fill_visible_row_range(&mut self, line: ScreenLine, columns: Range<u16>, fill: Cell) {
        let row: &mut [Cell] = &mut self[line];
        row[usize::from(columns.start)..usize::from(columns.end)].fill(fill);
    }

    /// Shifts one visible row's cells from `column` right by `count`
    /// columns, filling the columns that open with `fill`.
    ///
    /// The cells pushed past the last column are discarded. The row
    /// keeps its id, and nothing reaches history.
    ///
    /// The caller must clamp `count` to the columns from `column`
    /// through the row's end.
    pub fn insert_visible_row_cells(
        &mut self,
        line: ScreenLine,
        column: GridColumn,
        count: u16,
        fill: Cell,
    ) {
        let start = usize::from(column.0);
        let count = usize::from(count);
        let row: &mut [Cell] = &mut self[line];
        let cols = row.len();
        debug_assert!(
            start + count <= cols,
            "an in-row insert stays inside the row"
        );
        row.copy_within(start..cols - count, start + count);
        row[start..start + count].fill(fill);
    }

    /// Shifts one visible row's cells from `column + count` left to
    /// `column`, filling the columns that open at the row's end with
    /// `fill`.
    ///
    /// The `count` cells starting at `column` are overwritten by the
    /// cells that shift into them. The row keeps its id, and nothing
    /// reaches history.
    ///
    /// The caller must clamp `count` to the columns from `column`
    /// through the row's end.
    pub fn delete_visible_row_cells(
        &mut self,
        line: ScreenLine,
        column: GridColumn,
        count: u16,
        fill: Cell,
    ) {
        let start = usize::from(column.0);
        let count = usize::from(count);
        let row: &mut [Cell] = &mut self[line];
        let cols = row.len();
        debug_assert!(
            start + count <= cols,
            "an in-row delete stays inside the row"
        );
        row.copy_within(start + count..cols, start);
        row[cols - count..].fill(fill);
    }

    /// Scrolls the region up by one row: the row at `top` leaves and a
    /// `fill`-filled row enters at `bottom`.
    ///
    /// The departing row becomes the newest history row only when `top`
    /// is the first screen line. A region with content pinned above it
    /// discards the row instead.
    pub fn scroll_up_one(&mut self, top: ScreenLine, bottom: ScreenLine, fill: Cell) {
        let base = self.history_len();
        let id = self.mint();
        if top > ScreenLine(0) {
            let mut recycled = self
                .rows
                .remove(base + usize::from(top.0))
                .expect("the region's top row is inside the ring");
            recycled.id = id;
            recycled.cells.fill(fill);
            self.rows.insert(base + usize::from(bottom.0), recycled);
            return;
        }
        let grows_history = base < self.max_history;
        let departing = self.rows[base].id;
        let entering = if grows_history {
            GridRow {
                id,
                cells: Row::filled(self.size.cols, fill),
            }
        } else {
            let mut recycled = self
                .rows
                .pop_front()
                .expect("the ring always holds the visible rows");
            if self.max_history > 0 {
                self.history_index.pop_oldest(recycled.id);
            }
            recycled.id = id;
            recycled.cells.fill(fill);
            recycled
        };
        if self.max_history > 0 {
            self.history_index.enter(departing);
        }
        // NOTE: Seating the entering row just past the bottom margin is
        // what hands the departing row to history: the ring grows by one,
        // so the window of visible rows slides off it while the rows
        // below the margin keep their distance from the new end.
        let below_bottom = if grows_history {
            base + usize::from(bottom.0) + 1
        } else {
            base + usize::from(bottom.0)
        };
        self.rows.insert(below_bottom, entering);
    }

    /// Scrolls the region down by one row: a `fill`-filled row enters at
    /// `top` and the row at `bottom` is discarded.
    pub fn scroll_down_one(&mut self, top: ScreenLine, bottom: ScreenLine, fill: Cell) {
        let base = self.history_len();
        let mut recycled = self
            .rows
            .remove(base + usize::from(bottom.0))
            .expect("the region's bottom row is inside the ring");
        recycled.id = self.mint();
        recycled.cells.fill(fill);
        self.rows.insert(base + usize::from(top.0), recycled);
    }

    /// The id of the row at a screen line.
    pub fn line_id(&self, line: ScreenLine) -> LineId {
        self.rows[self.visible_index(line.0)].id
    }

    /// The id of the row at an active-grid line; `None` when the line
    /// is outside the ring.
    pub fn line_id_at(&self, line: GridLine) -> Option<LineId> {
        self.ring_index(line).map(|index| self.rows[index].id)
    }

    /// The id of the row a cell sits on; `None` when the cell's line is
    /// outside the ring or its column is past the width.
    pub fn line_id_at_point(&self, point: GridPoint) -> Option<LineId> {
        if point.column.0 >= self.size.cols {
            return None;
        }
        self.line_id_at(point.line)
    }

    /// The active-grid line the row `id` now sits at; `None` once it has
    /// left the ring.
    ///
    /// A history row resolves in constant time; a visible row costs a
    /// scan bounded by the screen height.
    pub fn grid_line(&self, id: LineId) -> Option<GridLine> {
        let history = self.history_len();
        if let Some(index) = self.history_index.index_of(id) {
            let line = index as i64 - history as i64;
            return Some(GridLine(
                i32::try_from(line).expect("a ring index minus its history fits in i32"),
            ));
        }
        self.rows
            .range(history..)
            .position(|row| row.id == id)
            .map(|line| GridLine(i32::try_from(line).expect("a screen line fits in i32")))
    }

    /// Borrows the row at an active-grid line; a negative line reaches
    /// into scrollback history.
    ///
    /// # Panics
    ///
    /// Panics unless the line resolves inside the ring, that is
    /// `-history_len <= line` and `line < rows`.
    pub fn row(&self, line: GridLine) -> &Row<Cell> {
        let index = self
            .ring_index(line)
            .expect("the line resolves inside the ring");
        &self.rows[index].cells
    }

    /// Number of history rows currently retained.
    pub fn history_len(&self) -> usize {
        self.rows.len() - usize::from(self.size.rows)
    }

    /// Resizes the grid, truncating rather than reflowing; returns
    /// whether the dimensions changed.
    ///
    /// A shrink drops rows from the bottom. Rows that should reach
    /// history must be scrolled off the top before this call.
    ///
    /// # Invariants
    ///
    /// Rows this appends carry freshly minted ids, so an anchor taken
    /// before the resize can never resolve to one of them.
    pub fn resize(&mut self, size: GridSize) -> bool {
        if self.size == size {
            return false;
        }
        // NOTE: the column pass runs first so the rows the row pass
        // appends are built at the target width; the other order builds
        // them at the old width and immediately reallocates each one.
        self.resize_cols(size.cols);
        self.resize_rows(size.rows);
        true
    }

    /// Appends one blank row at the live tail.
    ///
    /// # Invariants
    ///
    /// The row carries a freshly minted [`LineId`] that no anchor can
    /// already hold.
    fn push_blank_row(&mut self) {
        let id = self.mint();
        self.rows.push_back(GridRow {
            id,
            cells: Row::filled(self.size.cols, Cell::default()),
        });
    }

    fn resize_rows(&mut self, rows: u16) {
        let old = self.size.rows;
        if rows < old {
            let dropped = usize::from(old - rows);
            self.rows.truncate(self.rows.len() - dropped);
        } else if old < rows {
            let growth = usize::from(rows - old);
            let history = self.history_len();
            let reclaimed = growth.min(history);
            for row in self.rows.range(history - reclaimed..history) {
                self.history_index.reclaim_newest(row.id);
            }
            for _ in 0..growth - reclaimed {
                self.push_blank_row();
            }
        }
        self.size.rows = rows;
    }

    fn resize_cols(&mut self, cols: u16) {
        if self.size.cols == cols {
            return;
        }
        for row in &mut self.rows {
            row.cells.resize(cols, Cell::default());
        }
        self.size.cols = cols;
    }

    /// Hands out the next unused row id.
    ///
    /// # Invariants
    ///
    /// An id is never handed out twice in the grid's lifetime.
    fn mint(&mut self) -> LineId {
        let id = LineId(self.next_line_id);
        self.next_line_id = self
            .next_line_id
            .checked_add(1)
            .expect("a terminal cannot mint u64::MAX rows in one session");
        id
    }

    #[inline]
    fn visible_index(&self, line: u16) -> usize {
        self.history_len() + usize::from(line)
    }

    /// The ring index of an active-grid line; `None` outside the ring.
    fn ring_index(&self, line: GridLine) -> Option<usize> {
        let index = i64::from(self.history_len() as u32) + i64::from(line.0);
        let index = usize::try_from(index).ok()?;
        (index < self.rows.len()).then_some(index)
    }

    /// Checks that the history index names exactly the history rows, each
    /// at its ring index, and no visible row.
    #[cfg(test)]
    fn assert_history_index_matches_ring(&self) {
        let history = self.history_len();
        assert_eq!(self.history_index.len(), history);
        for (index, row) in self.rows.iter().enumerate() {
            let expected = (index < history).then_some(index);
            assert_eq!(self.history_index.index_of(row.id), expected);
        }
    }
}

/// Indexes the row at a screen line; history rows are unreachable
/// through it.
impl Index<ScreenLine> for Grid {
    type Output = Row<Cell>;

    fn index(&self, line: ScreenLine) -> &Row<Cell> {
        &self.rows[self.visible_index(line.0)].cells
    }
}

impl IndexMut<ScreenLine> for Grid {
    fn index_mut(&mut self, line: ScreenLine) -> &mut Row<Cell> {
        let index = self.visible_index(line.0);
        &mut self.rows[index].cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::Color;

    fn grid(rows: u16, max_history: usize) -> Grid {
        Grid::new(GridSize { cols: 4, rows }, max_history)
    }

    fn grid_with_history(history_rows: usize) -> Grid {
        let mut grid = Grid::new(GridSize { cols: 4, rows: 3 }, 10);
        for _ in 0..history_rows {
            grid.scroll_up_one(ScreenLine(0), ScreenLine(2), Cell::default());
        }
        grid
    }

    /// Scrolls with the margins a screen carries before any `DECSTBM`,
    /// which is the region every history assertion below is about.
    fn scroll_up_whole_screen(grid: &mut Grid, fill: Cell) {
        let bottom = ScreenLine(grid.size().rows - 1);
        grid.scroll_up_one(ScreenLine(0), bottom, fill);
    }

    /// Asserts that a screen line and a history line both resolve to an
    /// id that `grid_line` maps back to the same line.
    ///
    /// Case: the host names a cell by its active-grid line and the VT
    /// needs the row identity behind it, on the live screen and in
    /// scrollback alike.
    #[test]
    fn line_id_at_round_trips_through_grid_line() {
        let grid = grid_with_history(2);
        for line in [-2, -1, 0, 2] {
            let id = grid.line_id_at(GridLine(line)).expect("inside the ring");
            assert_eq!(grid.grid_line(id), Some(GridLine(line)));
        }
    }

    /// Asserts that a line above the retained history or below the
    /// screen resolves to `None`.
    ///
    /// Case: a request names a row the terminal has since trimmed.
    #[test]
    fn line_id_at_rejects_lines_outside_the_ring() {
        let grid = grid_with_history(2);
        assert_eq!(grid.line_id_at(GridLine(-3)), None);
        assert_eq!(grid.line_id_at(GridLine(3)), None);
    }

    /// Asserts that a resize to the size the grid already has reports
    /// no change and leaves the ring alone.
    ///
    /// Case: the window manager replays the same geometry after a
    /// focus change, so the host forwards a size the VT already holds.
    #[test]
    fn a_resize_to_the_current_size_reports_no_change() {
        let mut grid = grid(3, 10);
        grid[ScreenLine(0)][0].c = 'a';
        assert!(!grid.resize(GridSize { cols: 4, rows: 3 }));
        assert_eq!(grid[ScreenLine(0)][0].c, 'a');
    }

    /// Asserts that a shrink drops rows from the bottom and leaves the
    /// top ones on screen.
    ///
    /// Case: a short `ls` leaves its output near the top of a tall
    /// window and the user drags the window shorter.
    #[test]
    fn a_shrink_drops_rows_from_the_bottom() {
        let mut grid = grid(4, 10);
        grid[ScreenLine(0)][0].c = 'a';
        grid[ScreenLine(1)][0].c = 'b';
        assert!(grid.resize(GridSize { cols: 4, rows: 2 }));
        assert_eq!(grid.size().rows, 2);
        assert_eq!(grid.history_len(), 0);
        assert_eq!(grid[ScreenLine(0)][0].c, 'a');
        assert_eq!(grid[ScreenLine(1)][0].c, 'b');
    }

    /// Asserts that a shrink keeps the rows a preceding scroll handed
    /// to history and drops only the blanks that scroll left behind.
    ///
    /// Case: the shell's prompt sits on the last row of a tall window
    /// and the user drags the window shorter.
    #[test]
    fn a_shrink_after_a_scroll_keeps_what_the_scroll_saved() {
        let mut grid = grid(4, 10);
        for line in 0..4 {
            grid[ScreenLine(line)][0].c = char::from(b'a' + line as u8);
        }
        scroll_up_whole_screen(&mut grid, Cell::default());
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert!(grid.resize(GridSize { cols: 4, rows: 2 }));
        assert_eq!(grid.size().rows, 2);
        assert_eq!(grid.history_len(), 2);
        assert_eq!(grid[ScreenLine(0)][0].c, 'c');
        assert_eq!(grid[ScreenLine(1)][0].c, 'd');
        assert_eq!(grid.row(GridLine(-1))[0].c, 'b');
        assert_eq!(grid.row(GridLine(-2))[0].c, 'a');
    }

    /// Asserts that a shrink keeps the rows the preceding scrolls handed
    /// to history when those scrolls ran against a full history.
    ///
    /// Case: a long-running session has filled its scrollback to the
    /// cap when the user drags the window shorter.
    #[test]
    fn a_shrink_after_a_scroll_at_the_history_cap_keeps_what_the_scroll_saved() {
        let mut grid = grid(4, 2);
        for line in 0..4 {
            grid[ScreenLine(line)][0].c = char::from(b'a' + line as u8);
        }
        scroll_up_whole_screen(&mut grid, Cell::default());
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 2);
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 2);
        assert!(grid.resize(GridSize { cols: 4, rows: 2 }));
        assert_eq!(grid.size().rows, 2);
        assert_eq!(grid.history_len(), 2);
        assert_eq!(grid[ScreenLine(0)][0].c, 'd');
        assert_eq!(grid.row(GridLine(-1))[0].c, 'c');
    }

    /// Asserts that a growth pulls rows back out of history before it
    /// appends blank ones.
    ///
    /// Case: the user drags a window taller after scrolling output
    /// off the top.
    #[test]
    fn a_growth_reclaims_history_before_it_appends_blanks() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][0].c = 'a';
        grid[ScreenLine(1)][0].c = 'b';
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 1);
        assert!(grid.resize(GridSize { cols: 4, rows: 3 }));
        assert_eq!(grid.size().rows, 3);
        assert_eq!(grid.history_len(), 0);
        assert_eq!(grid[ScreenLine(0)][0].c, 'a');
        assert_eq!(grid[ScreenLine(1)][0].c, 'b');
    }

    /// Asserts that a growth beyond the history that exists appends
    /// blank rows at the bottom for the remainder.
    ///
    /// Case: the user drags a fresh window taller before any output
    /// has scrolled off, so there is no history to reclaim.
    #[test]
    fn a_growth_past_the_history_appends_blank_rows() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][0].c = 'a';
        assert!(grid.resize(GridSize { cols: 4, rows: 4 }));
        assert_eq!(grid.size().rows, 4);
        assert_eq!(grid.history_len(), 0);
        assert_eq!(grid[ScreenLine(0)][0].c, 'a');
        assert_eq!(grid[ScreenLine(3)][0], Cell::default());
    }

    /// Asserts that a narrowing truncates the history rows as well as
    /// the visible ones.
    ///
    /// Case: the user narrows a window whose earlier output has
    /// already scrolled into scrollback.
    #[test]
    fn a_narrowing_truncates_the_history_rows_too() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][3].c = 'x';
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 1);
        assert!(grid.resize(GridSize { cols: 2, rows: 2 }));
        assert_eq!(grid.row(GridLine(-1)).len(), 2);
        assert_eq!(grid[ScreenLine(0)].len(), 2);
    }

    /// Asserts that a widening pads every row in the ring with default
    /// cells.
    ///
    /// Case: the user widens a window whose earlier output has already
    /// scrolled into scrollback.
    #[test]
    fn a_widening_pads_every_row_in_the_ring() {
        let mut grid = grid(2, 10);
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert!(grid.resize(GridSize { cols: 6, rows: 2 }));
        assert_eq!(grid.row(GridLine(-1)).len(), 6);
        assert_eq!(grid[ScreenLine(0)].len(), 6);
        assert_eq!(grid[ScreenLine(0)][5], Cell::default());
    }

    /// Asserts that the rows a growth appends carry freshly minted
    /// ids rather than ids an anchor may still hold.
    ///
    /// Case: a webview is anchored to a row, the window shrinks so the
    /// row leaves, and the window grows again.
    #[test]
    fn a_growth_mints_fresh_ids_for_the_rows_it_appends() {
        let mut grid = grid(2, 10);
        let before = grid.line_id(ScreenLine(1));
        assert!(grid.resize(GridSize { cols: 4, rows: 4 }));
        assert_ne!(grid.line_id(ScreenLine(2)), before);
        assert_ne!(grid.line_id(ScreenLine(3)), before);
    }

    /// Asserts that grid line zero borrows the top row of the active
    /// screen, whatever history sits before it.
    ///
    /// Case: the emitter reads the first visible row of a terminal that
    /// has already scrolled output into scrollback.
    #[test]
    fn grid_line_zero_is_the_top_of_the_active_screen() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][0].c = 'a';
        scroll_up_whole_screen(&mut grid, Cell::default());
        grid[ScreenLine(0)][0].c = 'b';
        assert_eq!(grid.history_len(), 1);
        assert_eq!(grid.row(GridLine(0))[0].c, 'b');
    }

    /// Asserts that a negative grid line reaches the scrollback row
    /// that many lines above the active screen.
    ///
    /// Case: the user scrolls back and the emitter has to read rows
    /// that no longer sit in the visible window.
    #[test]
    fn a_negative_grid_line_reaches_into_history() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][0].c = 'a';
        scroll_up_whole_screen(&mut grid, Cell::default());
        grid[ScreenLine(0)][0].c = 'b';
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 2);
        assert_eq!(grid.row(GridLine(-1))[0].c, 'b');
        assert_eq!(grid.row(GridLine(-2))[0].c, 'a');
    }

    /// Asserts that a freshly built grid holds only blank visible rows
    /// and no history.
    ///
    /// Case: a terminal spawns and the shell has not written anything
    /// yet.
    #[test]
    fn a_fresh_grid_is_blank_with_no_history() {
        let grid = grid(3, 10);
        assert_eq!(grid.history_len(), 0);
        assert_eq!(grid.size(), GridSize { cols: 4, rows: 3 });
        assert_eq!(grid[ScreenLine(0)][0], Cell::default());
        assert_eq!(grid[ScreenLine(2)][3], Cell::default());
    }

    /// Asserts that a scroll below history capacity pushes the top
    /// visible row into history and fills the new bottom row with the
    /// given cell.
    ///
    /// Case: a shell at the bottom of the screen emits a newline while
    /// scrollback still has room.
    #[test]
    fn a_scroll_with_room_pushes_into_history() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][0].c = 'a';
        grid[ScreenLine(1)][0].c = 'b';
        let fill = Cell::blank_with_bg(Color::Indexed(4));
        scroll_up_whole_screen(&mut grid, fill);
        assert_eq!(grid.history_len(), 1);
        assert_eq!(grid[ScreenLine(0)][0].c, 'b');
        assert_eq!(grid[ScreenLine(1)][0], fill);
    }

    /// Asserts that a scroll at history capacity evicts the oldest row
    /// and keeps the history length at the cap.
    ///
    /// Case: a long-running shell session has filled the scrollback
    /// limit and keeps emitting newlines on the bottom line.
    #[test]
    fn a_scroll_at_capacity_evicts_the_oldest_row() {
        let mut grid = grid(2, 1);
        scroll_up_whole_screen(&mut grid, Cell::default());
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 1);
    }

    /// Asserts that a zero-capacity grid evicts on every scroll and
    /// keeps no history.
    ///
    /// Case: the user configures scrollback off and the shell emits a
    /// newline on the bottom line.
    #[test]
    fn zero_capacity_history_evicts_on_every_scroll() {
        let mut grid = grid(2, 0);
        grid[ScreenLine(0)][0].c = 'a';
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.history_len(), 0);
        assert_eq!(grid[ScreenLine(0)][0].c, ' ');
        assert_eq!(grid[ScreenLine(1)][0].c, ' ');
    }

    /// Asserts that a range fill overwrites exactly the given columns
    /// of one visible row.
    ///
    /// Case: an erase-in-line request clears part of a row while the
    /// neighboring cells keep their content.
    #[test]
    fn a_range_fill_overwrites_only_the_given_columns() {
        let mut grid = grid(2, 0);
        for column in 0..4 {
            grid[ScreenLine(0)][column].c = 'x';
        }
        grid.fill_visible_row_range(ScreenLine(0), 1..3, Cell::default());
        assert_eq!(grid[ScreenLine(0)][0].c, 'x');
        assert_eq!(grid[ScreenLine(0)][1].c, ' ');
        assert_eq!(grid[ScreenLine(0)][2].c, ' ');
        assert_eq!(grid[ScreenLine(0)][3].c, 'x');
    }

    /// Asserts that an insert moves the cells at and right of `column`
    /// up the row, drops the ones pushed past its end, fills the
    /// columns that open, mints no id, and leaves the neighboring rows
    /// and history untouched.
    ///
    /// Case: a line editor opens one column mid-row on a screen that
    /// has already scrolled once, with content on the rows either side
    /// of the edited one.
    #[test]
    fn an_insert_shifts_the_addressed_row_and_leaves_its_neighbors_alone() {
        let mut grid = grid_with_history(1);
        for column in 0..4 {
            grid[ScreenLine(1)][column].c = char::from(b'a' + column as u8);
        }
        grid[ScreenLine(0)][0].c = 'x';
        grid[ScreenLine(2)][0].c = 'y';
        let history_len = grid.history_len();
        let id = grid.line_id(ScreenLine(1));

        grid.insert_visible_row_cells(ScreenLine(1), GridColumn(1), 1, Cell::default());

        assert_eq!(grid[ScreenLine(1)][0].c, 'a');
        assert_eq!(grid[ScreenLine(1)][1].c, ' ');
        assert_eq!(grid[ScreenLine(1)][2].c, 'b');
        assert_eq!(grid[ScreenLine(1)][3].c, 'c');
        assert_eq!(grid[ScreenLine(0)][0].c, 'x');
        assert_eq!(grid[ScreenLine(2)][0].c, 'y');
        assert_eq!(grid.history_len(), history_len);
        assert_eq!(grid.line_id(ScreenLine(1)), id);
    }

    /// Asserts that a delete shifts the surviving cells down to
    /// `column`, fills the columns that open at the row's end, mints
    /// no id, and leaves the neighboring rows and history untouched.
    ///
    /// Case: a line editor closes a two-column gap mid-row on a screen
    /// that has already scrolled once, with content on the rows either
    /// side of the edited one.
    #[test]
    fn a_delete_shifts_the_addressed_row_and_leaves_its_neighbors_alone() {
        let mut grid = grid_with_history(1);
        for column in 0..4 {
            grid[ScreenLine(1)][column].c = char::from(b'a' + column as u8);
        }
        grid[ScreenLine(0)][0].c = 'x';
        grid[ScreenLine(2)][0].c = 'y';
        let history_len = grid.history_len();
        let id = grid.line_id(ScreenLine(1));

        grid.delete_visible_row_cells(ScreenLine(1), GridColumn(1), 2, Cell::default());

        assert_eq!(grid[ScreenLine(1)][0].c, 'a');
        assert_eq!(grid[ScreenLine(1)][1].c, 'd');
        assert_eq!(grid[ScreenLine(1)][2].c, ' ');
        assert_eq!(grid[ScreenLine(1)][3].c, ' ');
        assert_eq!(grid[ScreenLine(0)][0].c, 'x');
        assert_eq!(grid[ScreenLine(2)][0].c, 'y');
        assert_eq!(grid.history_len(), history_len);
        assert_eq!(grid.line_id(ScreenLine(1)), id);
    }

    /// Asserts that an id looked up from a screen line resolves back to the
    /// same grid line while the row is still on screen.
    ///
    /// Case: a webview mounts at the cursor and, before anything scrolls,
    /// the frame emitter projects it back to a row.
    #[test]
    fn a_line_id_round_trips_while_the_row_is_on_screen() {
        let grid = grid(3, 10);
        let id = grid.line_id(ScreenLine(1));
        assert_eq!(grid.grid_line(id), Some(GridLine(1)));
    }

    /// Asserts that a scroll moves a row's grid line down by one while its
    /// id keeps naming it.
    ///
    /// Case: a mounted webview stays anchored to its text as the shell
    /// keeps printing below it.
    #[test]
    fn an_id_follows_its_row_into_history() {
        let mut grid = grid(3, 10);
        let id = grid.line_id(ScreenLine(0));
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.grid_line(id), Some(GridLine(-1)));
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.grid_line(id), Some(GridLine(-2)));
    }

    /// Asserts that an id whose row was trimmed out of the ring no longer
    /// resolves.
    ///
    /// Case: the scrollback reaches its cap and the row a webview was
    /// anchored to is finally dropped.
    #[test]
    fn a_trimmed_id_no_longer_resolves() {
        let mut grid = grid(3, 1);
        let id = grid.line_id(ScreenLine(0));
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.grid_line(id), Some(GridLine(-1)));
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.grid_line(id), None);
    }

    /// Asserts that a grid without scrollback drops its front id on every
    /// scroll.
    ///
    /// Case: a full-screen application scrolls its alternate screen, where
    /// no row can be scrolled back to.
    #[test]
    fn a_grid_without_history_drops_the_front_id_on_every_scroll() {
        let mut grid = grid(3, 0);
        let id = grid.line_id(ScreenLine(0));
        scroll_up_whole_screen(&mut grid, Cell::default());
        assert_eq!(grid.grid_line(id), None);
    }

    /// Asserts that a freshly minted bottom row carries an id no earlier
    /// row shares.
    ///
    /// Case: output scrolls the screen and a blank row enters at the
    /// bottom.
    #[test]
    fn a_scroll_mints_a_fresh_id_for_the_incoming_row() {
        let mut grid = grid(3, 10);
        let before = grid.line_id(ScreenLine(2));
        scroll_up_whole_screen(&mut grid, Cell::default());
        let after = grid.line_id(ScreenLine(2));
        assert_ne!(before, after);
        assert_eq!(grid.grid_line(before), Some(GridLine(1)));
    }

    mod scroll_down {
        use super::*;

        /// A grid whose visible rows are labelled `a`, `b`, `c`, … in
        /// column zero.
        fn labelled(rows: u16, max_history: usize) -> Grid {
            let mut grid = grid(rows, max_history);
            for line in 0..rows {
                grid[ScreenLine(line)][0].c = char::from(b'a' + line as u8);
            }
            grid
        }

        /// The full-screen region of a grid `rows` rows tall.
        fn whole(rows: u16) -> (ScreenLine, ScreenLine) {
            (ScreenLine(0), ScreenLine(rows - 1))
        }

        /// Asserts that a reverse scroll moves the region's content down
        /// one row and drops the row that falls off its bottom.
        ///
        /// Case: a full-screen pager scrolls backwards past the first line
        /// it is showing.
        #[test]
        fn a_reverse_scroll_moves_content_down_and_drops_the_bottom_row() {
            let mut grid = labelled(3, 10);
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, Cell::default());
            assert_eq!(grid[ScreenLine(0)][0].c, Cell::default().c);
            assert_eq!(grid[ScreenLine(1)][0].c, 'a');
            assert_eq!(grid[ScreenLine(2)][0].c, 'b');
        }

        /// Asserts that a reverse scroll leaves the history untouched,
        /// neither pushing the row that falls off the bottom into it nor
        /// pulling its newest row back.
        ///
        /// Case: a full-screen editor scrolls its view backwards while the
        /// shell's earlier output still sits in scrollback behind it.
        #[test]
        fn a_reverse_scroll_leaves_history_alone() {
            let mut grid = labelled(3, 10);
            scroll_up_whole_screen(&mut grid, Cell::default());
            let history_len = grid.history_len();
            let oldest = grid.row(GridLine(-1))[0].c;
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, Cell::default());
            assert_eq!(grid.history_len(), history_len);
            assert_eq!(grid.row(GridLine(-1))[0].c, oldest);
        }

        /// Asserts that the row entering at the top carries the fill.
        ///
        /// Case: an application sets a background colour and scrolls
        /// backwards.
        #[test]
        fn the_row_entering_at_the_top_carries_the_fill() {
            let mut grid = labelled(3, 10);
            let fill = Cell::blank_with_bg(Color::Indexed(4));
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, fill);
            assert_eq!(grid[ScreenLine(0)][0], fill);
        }

        /// Asserts that the row entering at the top carries an id no
        /// surviving row shares.
        ///
        /// Case: a webview is anchored to a row and the screen scrolls
        /// backwards, leaving a blank row where that row used to sit.
        #[test]
        fn the_incoming_row_carries_an_id_no_surviving_row_shares() {
            let mut grid = grid(3, 10);
            let before: Vec<LineId> = (0..3).map(|l| grid.line_id(ScreenLine(l))).collect();
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, Cell::default());
            assert!(!before.contains(&grid.line_id(ScreenLine(0))));
        }

        /// Asserts that the discarded row's id stops resolving.
        ///
        /// Case: a webview sits on the last row of the screen, and a
        /// reverse scroll pushes that row off the bottom.
        #[test]
        fn the_discarded_rows_id_stops_resolving() {
            let mut grid = grid(3, 10);
            let discarded = grid.line_id(ScreenLine(2));
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, Cell::default());
            assert_eq!(grid.grid_line(discarded), None);
        }

        /// Asserts that a surviving row's id resolves one line lower.
        ///
        /// Case: a webview sits on the top row, and a reverse scroll
        /// pushes it down to make room for the blank.
        #[test]
        fn a_surviving_id_resolves_one_row_lower() {
            let mut grid = grid(3, 10);
            let survivor = grid.line_id(ScreenLine(0));
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, Cell::default());
            assert_eq!(grid.grid_line(survivor), Some(GridLine(1)));
        }

        /// Asserts that a grid built without history scrolls down without
        /// underflowing.
        ///
        /// Case: a full-screen application scrolls backwards on the
        /// alternate screen, which is constructed with no scrollback at
        /// all.
        #[test]
        fn a_grid_without_history_scrolls_down() {
            let mut grid = labelled(3, 0);
            let (top, bottom) = whole(3);
            grid.scroll_down_one(top, bottom, Cell::default());
            assert_eq!(grid.history_len(), 0);
            assert_eq!(grid[ScreenLine(1)][0].c, 'a');
        }

        /// Asserts that a one-row screen replaces its only row.
        ///
        /// Case: the user drags the window down to a single line and the
        /// program running in it scrolls backwards.
        #[test]
        fn a_one_row_screen_replaces_its_only_row() {
            let mut grid = labelled(1, 10);
            let only = grid.line_id(ScreenLine(0));
            grid.scroll_down_one(ScreenLine(0), ScreenLine(0), Cell::default());
            assert_eq!(grid.grid_line(only), None);
            assert_eq!(grid[ScreenLine(0)][0].c, Cell::default().c);
        }

        /// Asserts that a region narrower than the screen moves only its
        /// own rows and discards only its own bottom row.
        ///
        /// Case: an application sets a scroll region for a pager pane and
        /// scrolls it backwards while a status line sits above it.
        #[test]
        fn a_narrow_region_moves_only_its_own_rows() {
            let mut grid = labelled(4, 10);
            grid.scroll_down_one(ScreenLine(1), ScreenLine(2), Cell::default());
            assert_eq!(grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(grid[ScreenLine(1)][0].c, Cell::default().c);
            assert_eq!(grid[ScreenLine(2)][0].c, 'b');
            assert_eq!(grid[ScreenLine(3)][0].c, 'd');
        }

        /// Asserts that an anchor still resolves once the history holds
        /// ids that are no longer ascending.
        ///
        /// Case: a full-screen application scrolls backwards, and output
        /// then pushes the row that scroll inserted into history ahead of
        /// the rows it was inserted above.
        #[test]
        fn an_anchor_still_resolves_once_the_history_ids_are_unordered() {
            let mut grid = grid(3, 10);
            let anchor = grid.line_id(ScreenLine::TOP);
            grid.scroll_down_one(ScreenLine(0), ScreenLine(2), Cell::default());
            for _ in 0..3 {
                scroll_up_whole_screen(&mut grid, Cell::default());
            }
            assert_eq!(grid.grid_line(anchor), Some(GridLine(-2)));
        }
    }

    mod reset {
        use super::*;

        /// Asserts that a reset leaves the grid blank with no history.
        ///
        /// Case: the shell sends `RIS` to a terminal that has scrolled a
        /// build log into its scrollback.
        #[test]
        fn a_reset_blanks_the_grid_and_drops_the_history() {
            let mut grid = grid(3, 10);
            grid[ScreenLine(0)][0].c = 'a';
            scroll_up_whole_screen(&mut grid, Cell::default());
            grid[ScreenLine(0)][0].c = 'b';
            grid.reset();
            assert_eq!(grid.history_len(), 0);
            assert!(grid.is_blank());
        }

        /// Asserts that a reset mints ids no anchor taken before it can
        /// match, rather than renumbering the rows from zero.
        ///
        /// Case: a webview is anchored to a row when `RIS` arrives, and
        /// the placement store has yet to sweep its lost anchors.
        #[test]
        fn a_reset_mints_ids_no_pre_reset_anchor_can_match() {
            let mut grid = grid(3, 10);
            let anchor = grid.line_id(ScreenLine(0));
            grid.reset();
            assert_eq!(grid.grid_line(anchor), None);
        }

        /// Asserts that the id counter keeps moving forward across
        /// repeated resets.
        ///
        /// Case: an application sends `RIS` twice while a webview from
        /// before the first one is still mounted.
        #[test]
        fn a_reset_does_not_rewind_the_id_counter() {
            let mut grid = grid(3, 10);
            grid.reset();
            let first: Vec<LineId> = (0..3).map(|line| grid.line_id(ScreenLine(line))).collect();
            grid.reset();
            let second: Vec<LineId> = (0..3).map(|line| grid.line_id(ScreenLine(line))).collect();
            assert!(second.iter().all(|id| !first.contains(id)));
            for id in &first {
                assert_eq!(grid.grid_line(*id), None);
            }
        }
    }

    mod history_index {
        use super::*;

        fn ids(grid: &Grid) -> Vec<LineId> {
            (0..grid.size().rows)
                .map(|line| grid.line_id(ScreenLine(line)))
                .collect()
        }

        /// Asserts that a growth brings the newest history row back onto
        /// the screen at line zero while the older ones stay in history.
        ///
        /// Case: two lines have scrolled into history and the user drags
        /// the window one row taller.
        #[test]
        fn a_growth_reclaims_the_newest_history_row() {
            let mut grid = grid(2, 10);
            let [a, b] = ids(&grid)[..] else {
                unreachable!()
            };
            scroll_up_whole_screen(&mut grid, Cell::default());
            scroll_up_whole_screen(&mut grid, Cell::default());
            assert!(grid.resize(GridSize { cols: 4, rows: 3 }));
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(0)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that one growth reclaiming several rows resolves each
        /// of them to consecutive screen lines.
        ///
        /// Case: three lines sit in history and the user drags the
        /// window two rows taller in one motion.
        #[test]
        fn a_growth_reclaims_several_rows_in_order() {
            let mut grid = grid(2, 10);
            let [a, b] = ids(&grid)[..] else {
                unreachable!()
            };
            scroll_up_whole_screen(&mut grid, Cell::default());
            let c = grid.line_id(ScreenLine(1));
            scroll_up_whole_screen(&mut grid, Cell::default());
            scroll_up_whole_screen(&mut grid, Cell::default());
            assert_eq!(grid.history_len(), 3);
            assert!(grid.resize(GridSize { cols: 4, rows: 4 }));
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(0)));
            assert_eq!(grid.grid_line(c), Some(GridLine(1)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a growth larger than the history reclaims every
        /// history row and appends fresh ones below.
        ///
        /// Case: one line sits in history and the user maximizes the
        /// window, asking for far more rows than history can supply.
        #[test]
        fn a_growth_larger_than_the_history_reclaims_it_all() {
            let mut grid = grid(2, 10);
            let [a, b] = ids(&grid)[..] else {
                unreachable!()
            };
            scroll_up_whole_screen(&mut grid, Cell::default());
            assert!(grid.resize(GridSize { cols: 4, rows: 5 }));
            assert_eq!(grid.history_len(), 0);
            assert_eq!(grid.grid_line(a), Some(GridLine(0)));
            assert_eq!(grid.grid_line(b), Some(GridLine(1)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a row reclaimed after the cap has popped rows
        /// re-enters history at the right line when output scrolls again.
        ///
        /// Case: on a terminal at its scrollback cap the user drags the
        /// window taller and the shell then prints another line.
        #[test]
        fn a_reclaimed_row_re_enters_history_after_the_cap_has_popped() {
            let mut grid = grid(3, 2);
            let [a, b, c] = ids(&grid)[..] else {
                unreachable!()
            };
            for _ in 0..3 {
                scroll_up_whole_screen(&mut grid, Cell::default());
            }
            assert_eq!(grid.grid_line(a), None);
            assert!(grid.resize(GridSize { cols: 4, rows: 4 }));
            assert_eq!(grid.grid_line(b), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(c), Some(GridLine(0)));
            grid.assert_history_index_matches_ring();
            scroll_up_whole_screen(&mut grid, Cell::default());
            assert_eq!(grid.grid_line(b), Some(GridLine(-2)));
            assert_eq!(grid.grid_line(c), Some(GridLine(-1)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a shrink stops the dropped bottom rows from
        /// resolving and leaves history where it was.
        ///
        /// Case: one line sits in history and the user drags the window
        /// two rows shorter with the cursor near the top.
        #[test]
        fn a_shrink_drops_the_bottom_ids_and_keeps_history() {
            let mut grid = grid(4, 10);
            let a = grid.line_id(ScreenLine(0));
            scroll_up_whole_screen(&mut grid, Cell::default());
            let [b, c, d, e] = ids(&grid)[..] else {
                unreachable!()
            };
            assert!(grid.resize(GridSize { cols: 4, rows: 2 }));
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(0)));
            assert_eq!(grid.grid_line(c), Some(GridLine(1)));
            assert_eq!(grid.grid_line(d), None);
            assert_eq!(grid.grid_line(e), None);
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a columns-only resize leaves every id where it was.
        ///
        /// Case: one line sits in history and the user drags the window
        /// wider without changing its height.
        #[test]
        fn a_columns_only_resize_moves_no_id() {
            let mut grid = grid(3, 10);
            let a = grid.line_id(ScreenLine(0));
            scroll_up_whole_screen(&mut grid, Cell::default());
            let b = grid.line_id(ScreenLine(0));
            assert!(grid.resize(GridSize { cols: 6, rows: 3 }));
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(0)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a scroll whose region starts at the top but ends
        /// above the last line still hands the top row to history while
        /// the rows below the region keep their lines.
        ///
        /// Case: an application pins a status line to the bottom row with
        /// `DECSTBM` and the pane above it scrolls.
        #[test]
        fn a_top_anchored_region_scroll_hands_the_top_row_to_history() {
            let mut grid = grid(4, 10);
            let [a, b, _, d] = ids(&grid)[..] else {
                unreachable!()
            };
            grid.scroll_up_one(ScreenLine(0), ScreenLine(2), Cell::default());
            assert_eq!(grid.history_len(), 1);
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(0)));
            assert_eq!(grid.grid_line(d), Some(GridLine(3)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a scroll inside a region below the top discards
        /// the region's top row without touching history.
        ///
        /// Case: a line has scrolled into history, then an application
        /// sets a scroll region under a header line and scrolls it.
        #[test]
        fn a_region_scroll_below_the_top_discards_without_touching_history() {
            let mut grid = grid(4, 10);
            let a = grid.line_id(ScreenLine(0));
            scroll_up_whole_screen(&mut grid, Cell::default());
            let [b, c, d, e] = ids(&grid)[..] else {
                unreachable!()
            };
            grid.scroll_up_one(ScreenLine(1), ScreenLine(2), Cell::default());
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(0)));
            assert_eq!(grid.grid_line(c), None);
            assert_eq!(grid.grid_line(d), Some(GridLine(1)));
            assert_eq!(grid.grid_line(e), Some(GridLine(3)));
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a reverse scroll after history exists discards the
        /// bottom row without touching history.
        ///
        /// Case: a line has scrolled into history and an application then
        /// scrolls the whole screen backwards.
        #[test]
        fn a_reverse_scroll_after_history_exists_leaves_history_alone() {
            let mut grid = grid(3, 10);
            let a = grid.line_id(ScreenLine(0));
            scroll_up_whole_screen(&mut grid, Cell::default());
            let [b, _, d] = ids(&grid)[..] else {
                unreachable!()
            };
            grid.scroll_down_one(ScreenLine(0), ScreenLine(2), Cell::default());
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            assert_eq!(grid.grid_line(b), Some(GridLine(1)));
            assert_eq!(grid.grid_line(d), None);
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that scrolling after a reset fills history afresh and
        /// a second reset empties it again.
        ///
        /// Case: an application sends `RIS`, prints a screenful, and
        /// sends `RIS` again.
        #[test]
        fn history_refills_between_repeated_resets() {
            let mut grid = grid(3, 10);
            scroll_up_whole_screen(&mut grid, Cell::default());
            grid.reset();
            grid.assert_history_index_matches_ring();
            let a = grid.line_id(ScreenLine(0));
            scroll_up_whole_screen(&mut grid, Cell::default());
            assert_eq!(grid.grid_line(a), Some(GridLine(-1)));
            grid.assert_history_index_matches_ring();
            grid.reset();
            assert_eq!(grid.grid_line(a), None);
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that a grid without history never indexes a row.
        ///
        /// Case: a full-screen application scrolls its alternate screen
        /// forwards and backwards.
        #[test]
        fn a_grid_without_history_indexes_nothing() {
            let mut grid = grid(3, 0);
            scroll_up_whole_screen(&mut grid, Cell::default());
            grid.scroll_down_one(ScreenLine(0), ScreenLine(2), Cell::default());
            scroll_up_whole_screen(&mut grid, Cell::default());
            assert_eq!(grid.history_len(), 0);
            grid.assert_history_index_matches_ring();
        }

        /// Asserts that the index agrees with the ring after every step of
        /// a session that mixes every ring mutation.
        ///
        /// Case: a long session scrolls to the cap, keeps scrolling, runs
        /// a full-screen pager with region scrolls, resizes both ways,
        /// resets, and scrolls again.
        #[test]
        fn the_index_agrees_with_the_ring_through_a_mixed_session() {
            let mut grid = grid(4, 3);
            let mut seen: Vec<LineId> = ids(&grid);
            let mut step = |grid: &mut Grid| {
                grid.assert_history_index_matches_ring();
                for id in &seen {
                    if let Some(line) = grid.grid_line(*id) {
                        let index = usize::try_from(i64::from(line.0) + grid.history_len() as i64)
                            .expect("a resolved line sits inside the ring");
                        assert_eq!(grid.rows[index].id, *id);
                    }
                }
                seen.extend(ids(grid));
            };
            for _ in 0..5 {
                scroll_up_whole_screen(&mut grid, Cell::default());
                step(&mut grid);
            }
            grid.scroll_up_one(ScreenLine(1), ScreenLine(2), Cell::default());
            step(&mut grid);
            grid.scroll_down_one(ScreenLine(0), ScreenLine(3), Cell::default());
            step(&mut grid);
            grid.scroll_up_one(ScreenLine(0), ScreenLine(2), Cell::default());
            step(&mut grid);
            assert!(grid.resize(GridSize { cols: 4, rows: 6 }));
            step(&mut grid);
            assert!(grid.resize(GridSize { cols: 6, rows: 6 }));
            step(&mut grid);
            assert!(grid.resize(GridSize { cols: 6, rows: 2 }));
            step(&mut grid);
            for _ in 0..4 {
                scroll_up_whole_screen(&mut grid, Cell::default());
                step(&mut grid);
            }
            grid.reset();
            step(&mut grid);
            scroll_up_whole_screen(&mut grid, Cell::default());
            step(&mut grid);
        }
    }
}
