//! Cell storage: the visible screen plus the scrollback ring.

use crate::schema::GridSize;
use crate::screen::cell::Cell;
use std::collections::VecDeque;
use std::ops::Range;

/// History effect of one bottom-line scroll, for placement bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryEvent {
    /// The top visible row moved into scrollback history.
    Pushed,
    /// The push also evicted the oldest history row (ring at capacity).
    PushedWithEviction,
}

/// A single storage row of cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Row(Vec<Cell>);

impl Row {
    fn filled(cols: u16, fill: Cell) -> Self {
        Self(vec![fill; usize::from(cols)])
    }
}

/// Storage-only grid: scrollback history plus the visible screen in
/// one ring.
///
/// The last `size.rows` entries of `rows` are the visible screen;
/// every entry before them is history, oldest first. The grid knows
/// nothing about cursors, pens, or viewports.
#[derive(Debug)]
pub struct Grid {
    rows: VecDeque<Row>,
    size: GridSize,
    max_history: usize,
}

impl Grid {
    /// Builds a grid of blank visible rows with an empty history.
    pub fn build(size: GridSize, max_history: usize) -> Self {
        let mut rows = VecDeque::with_capacity(usize::from(size.rows));
        for _ in 0..size.rows {
            rows.push_back(Row::filled(size.cols, Cell::default()));
        }
        Self {
            rows,
            size,
            max_history,
        }
    }

    /// Grid dimensions in cells.
    pub fn size(&self) -> GridSize {
        self.size
    }

    /// Reads the cell at visible-screen coordinates.
    pub fn cell(&self, line: u16, column: u16) -> &Cell {
        &self.rows[self.visible_index(line)].0[usize::from(column)]
    }

    /// Mutably borrows the cell at visible-screen coordinates.
    pub fn cell_mut(&mut self, line: u16, column: u16) -> &mut Cell {
        let index = self.visible_index(line);
        &mut self.rows[index].0[usize::from(column)]
    }

    /// Overwrites the given column range of one visible row with `fill`.
    pub fn fill_visible_row_range(&mut self, line: u16, columns: Range<u16>, fill: Cell) {
        let index = self.visible_index(line);
        self.rows[index].0[usize::from(columns.start)..usize::from(columns.end)].fill(fill);
    }

    /// Scrolls the visible screen up by one row: the top visible row
    /// becomes the newest history row and a `fill`-filled row enters at
    /// the bottom. At capacity the evicted row's allocation is
    /// recycled as the incoming row.
    pub fn scroll_up_one(&mut self, fill: Cell) -> HistoryEvent {
        if self.history_len() < self.max_history {
            self.rows.push_back(Row::filled(self.size.cols, fill));
            return HistoryEvent::Pushed;
        }
        let mut recycled = self
            .rows
            .pop_front()
            .expect("the ring always holds the visible rows");
        recycled.0.fill(fill);
        self.rows.push_back(recycled);
        HistoryEvent::PushedWithEviction
    }

    /// Number of history rows currently retained.
    pub fn history_len(&self) -> usize {
        self.rows.len() - usize::from(self.size.rows)
    }

    fn visible_index(&self, line: u16) -> usize {
        self.history_len() + usize::from(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Color;

    fn grid(rows: u16, max_history: usize) -> Grid {
        Grid::build(
            GridSize { cols: 4, rows },
            max_history,
        )
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
        assert_eq!(*grid.cell(0, 0), Cell::default());
        assert_eq!(*grid.cell(2, 3), Cell::default());
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
        grid.cell_mut(0, 0).c = 'a';
        grid.cell_mut(1, 0).c = 'b';
        let fill = Cell::blank_with_bg(Color::Indexed(4));
        assert_eq!(grid.scroll_up_one(fill), HistoryEvent::Pushed);
        assert_eq!(grid.history_len(), 1);
        assert_eq!(grid.cell(0, 0).c, 'b');
        assert_eq!(*grid.cell(1, 0), fill);
    }

    /// Asserts that a scroll at history capacity reports the eviction
    /// and keeps the history length at the cap.
    ///
    /// Case: a long-running shell session has filled the scrollback
    /// limit, and every further bottom-line newline drops the oldest
    /// history line.
    #[test]
    fn a_scroll_at_capacity_evicts_the_oldest_row() {
        let mut grid = grid(2, 1);
        assert_eq!(grid.scroll_up_one(Cell::default()), HistoryEvent::Pushed);
        assert_eq!(
            grid.scroll_up_one(Cell::default()),
            HistoryEvent::PushedWithEviction
        );
        assert_eq!(grid.history_len(), 1);
    }

    /// Asserts that a zero-capacity grid reports every scroll as an
    /// eviction and keeps no history.
    ///
    /// Case: the user configures scrollback off, so a bottom-line
    /// newline discards the top visible row outright.
    #[test]
    fn zero_capacity_history_evicts_on_every_scroll() {
        let mut grid = grid(2, 0);
        grid.cell_mut(0, 0).c = 'a';
        assert_eq!(
            grid.scroll_up_one(Cell::default()),
            HistoryEvent::PushedWithEviction
        );
        assert_eq!(grid.history_len(), 0);
        assert_eq!(grid.cell(0, 0).c, ' ');
        assert_eq!(grid.cell(1, 0).c, ' ');
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
            grid.cell_mut(0, column).c = 'x';
        }
        grid.fill_visible_row_range(0, 1..3, Cell::default());
        assert_eq!(grid.cell(0, 0).c, 'x');
        assert_eq!(grid.cell(0, 1).c, ' ');
        assert_eq!(grid.cell(0, 2).c, ' ');
        assert_eq!(grid.cell(0, 3).c, 'x');
    }
}
