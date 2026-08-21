//! Cell storage: the visible screen plus the scrollback ring.

pub mod row;
pub mod run;

use crate::schema::{GridLine, GridSize, ScreenLine};
use crate::screen::cell::Cell;
use crate::screen::grid::row::Row;
use std::collections::VecDeque;
use std::ops::{Index, IndexMut, Range};

/// Stable identity of one grid row, minted when the row enters the ring.
///
/// # Invariants
///
/// Ids are minted monotonically per grid and never reused, and the ring
/// holds a consecutive run of them, oldest at the front. A placement
/// anchored to one can therefore never be re-pointed at later content
/// on the same grid.
///
/// The uniqueness is per grid, NOT per terminal: the primary and
/// alternate screens own separate grids that both start at zero, so
/// resolving an id against the wrong one silently names a different row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LineId(u64);

/// Storage-only grid: scrollback history plus the visible screen in
/// one ring.
///
/// The grid knows nothing about cursors, pens, or viewports.
#[derive(Debug)]
pub struct Grid {
    /// One logical ring holding history and the active screen:
    /// the last `size.rows` entries are the active screen, everything
    /// before them is history, oldest first. Indices are logical
    /// (`VecDeque` hides the physical rotation), so index `0` is
    /// always the oldest surviving history row and the boundary sits
    /// at `history_len`.
    rows: VecDeque<Row<Cell>>,
    /// Active-screen dimensions; `rows` always keeps at least this
    /// many entries as its tail window.
    size: GridSize,
    /// History row cap: `history_len` never exceeds it, and a scroll
    /// at the cap recycles the evicted row as the incoming blank.
    max_history: usize,
    /// Id of the ring's front row — the oldest surviving line.
    ///
    /// Every row's id is `front_line_id + its index in the ring`, because
    /// ids are minted at the back and dropped from the front, so the ring
    /// always holds a consecutive run.
    front_line_id: u64,
}

impl Grid {
    /// Builds a grid of blank visible rows with an empty history.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        let mut rows = VecDeque::with_capacity(usize::from(size.rows));
        for _ in 0..size.rows {
            rows.push_back(Row::filled(size.cols, Cell::default()));
        }
        Self {
            rows,
            size,
            max_history,
            front_line_id: 0,
        }
    }

    /// Grid dimensions in cells.
    pub const fn size(&self) -> GridSize {
        self.size
    }

    /// Overwrites the given column range of one visible row with `fill`.
    pub fn fill_visible_row_range(&mut self, line: u16, columns: Range<u16>, fill: Cell) {
        let index = self.visible_index(line);
        // NOTE: `Row`'s own `Index<u16>` shadows the slice's range
        // indexing, so the row has to reach the slice through `DerefMut`
        // before a range can be applied.
        let row: &mut [Cell] = &mut self.rows[index];
        row[usize::from(columns.start)..usize::from(columns.end)].fill(fill);
    }

    /// Scrolls the visible screen up by one row: the top visible row
    /// becomes the newest history row and a `fill`-filled row enters at
    /// the bottom.
    pub fn scroll_up_one(&mut self, fill: Cell) {
        if self.history_len() < self.max_history {
            self.rows.push_back(Row::filled(self.size.cols, fill));
            return;
        }
        let mut recycled = self
            .rows
            .pop_front()
            .expect("the ring always holds the visible rows");
        self.front_line_id = self
            .front_line_id
            .checked_add(1)
            .expect("a terminal cannot scroll u64::MAX rows in one session");
        recycled.fill(fill);
        self.rows.push_back(recycled);
    }

    /// The id of the row at a screen line.
    pub(super) fn line_id(&self, line: ScreenLine) -> LineId {
        LineId(self.front_line_id + self.history_len() as u64 + u64::from(line.0))
    }

    /// The active-grid line the row `id` now sits at; `None` once it has
    /// left the ring.
    pub(super) fn grid_line(&self, id: LineId) -> Option<GridLine> {
        let index = id.0.checked_sub(self.front_line_id)?;
        if index >= self.rows.len() as u64 {
            return None;
        }
        let line = index as i64 - self.history_len() as i64;
        Some(GridLine(
            i32::try_from(line).expect("a ring index minus its history fits in i32"),
        ))
    }

    /// Borrows the row at an active-grid line; a negative line reaches
    /// into scrollback history.
    ///
    /// # Invariants
    ///
    /// The line must resolve inside the ring — `-history_len <= line`
    /// and `line < rows`. [`crate::screen::Screen`] guarantees that by
    /// clamping the viewport to the history it actually has.
    pub(super) fn row(&self, line: GridLine) -> &Row<Cell> {
        let index = i64::from(self.history_len() as u32) + i64::from(line.0);
        let index = usize::try_from(index).expect("the line resolves inside the ring");
        &self.rows[index]
    }

    /// Number of history rows currently retained.
    pub fn history_len(&self) -> usize {
        self.rows.len() - usize::from(self.size.rows)
    }

    #[inline]
    fn visible_index(&self, line: u16) -> usize {
        self.history_len() + usize::from(line)
    }
}

/// Indexes the row at a screen line; history rows are structurally
/// unreachable because [`ScreenLine`] cannot be negative.
impl Index<ScreenLine> for Grid {
    type Output = Row<Cell>;

    fn index(&self, line: ScreenLine) -> &Row<Cell> {
        &self.rows[self.visible_index(line.0)]
    }
}

impl IndexMut<ScreenLine> for Grid {
    fn index_mut(&mut self, line: ScreenLine) -> &mut Row<Cell> {
        let index = self.visible_index(line.0);
        &mut self.rows[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Color;

    fn grid(rows: u16, max_history: usize) -> Grid {
        Grid::new(GridSize { cols: 4, rows }, max_history)
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
        grid.scroll_up_one(Cell::default());
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
        grid.scroll_up_one(Cell::default());
        grid[ScreenLine(0)][0].c = 'b';
        grid.scroll_up_one(Cell::default());
        assert_eq!(grid.history_len(), 2);
        assert_eq!(grid.row(GridLine(-1))[0].c, 'b');
        assert_eq!(grid.row(GridLine(-2))[0].c, 'a');
    }

    /// Asserts that a freshly built grid holds only blank visible rows
    /// and no history.
    ///
    /// Case: a terminal spawns and the shell has not written anything
    /// yet, so the whole screen shows default blanks.
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
    /// scrollback still has room, so the oldest visible line becomes
    /// history instead of disappearing.
    #[test]
    fn a_scroll_with_room_pushes_into_history() {
        let mut grid = grid(2, 10);
        grid[ScreenLine(0)][0].c = 'a';
        grid[ScreenLine(1)][0].c = 'b';
        let fill = Cell::blank_with_bg(Color::Indexed(4));
        grid.scroll_up_one(fill);
        assert_eq!(grid.history_len(), 1);
        assert_eq!(grid[ScreenLine(0)][0].c, 'b');
        assert_eq!(grid[ScreenLine(1)][0], fill);
    }

    /// Asserts that a scroll at history capacity evicts the oldest row
    /// and keeps the history length at the cap.
    ///
    /// Case: a long-running shell session has filled the scrollback
    /// limit, and every further bottom-line newline drops the oldest
    /// history line.
    #[test]
    fn a_scroll_at_capacity_evicts_the_oldest_row() {
        let mut grid = grid(2, 1);
        grid.scroll_up_one(Cell::default());
        grid.scroll_up_one(Cell::default());
        assert_eq!(grid.history_len(), 1);
    }

    /// Asserts that a zero-capacity grid evicts on every scroll and
    /// keeps no history.
    ///
    /// Case: the user configures scrollback off, so a bottom-line
    /// newline discards the top visible row outright.
    #[test]
    fn zero_capacity_history_evicts_on_every_scroll() {
        let mut grid = grid(2, 0);
        grid[ScreenLine(0)][0].c = 'a';
        grid.scroll_up_one(Cell::default());
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
        grid.fill_visible_row_range(0, 1..3, Cell::default());
        assert_eq!(grid[ScreenLine(0)][0].c, 'x');
        assert_eq!(grid[ScreenLine(0)][1].c, ' ');
        assert_eq!(grid[ScreenLine(0)][2].c, ' ');
        assert_eq!(grid[ScreenLine(0)][3].c, 'x');
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
        grid.scroll_up_one(Cell::default());
        assert_eq!(grid.grid_line(id), Some(GridLine(-1)));
        grid.scroll_up_one(Cell::default());
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
        grid.scroll_up_one(Cell::default());
        assert_eq!(grid.grid_line(id), Some(GridLine(-1)));
        grid.scroll_up_one(Cell::default());
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
        grid.scroll_up_one(Cell::default());
        assert_eq!(grid.grid_line(id), None);
    }

    /// Asserts that a freshly minted bottom row carries an id no earlier
    /// row shares.
    ///
    /// Case: output scrolls the screen and the blank row entering at the
    /// bottom must not inherit the identity of the row that left.
    #[test]
    fn a_scroll_mints_a_fresh_id_for_the_incoming_row() {
        let mut grid = grid(3, 10);
        let before = grid.line_id(ScreenLine(2));
        grid.scroll_up_one(Cell::default());
        let after = grid.line_id(ScreenLine(2));
        assert_ne!(before, after);
        assert_eq!(grid.grid_line(before), Some(GridLine(1)));
    }
}
