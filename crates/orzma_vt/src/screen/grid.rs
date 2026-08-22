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
/// An id is the row's identity, not its address: it follows the row
/// wherever the row moves inside the ring. Ids are minted monotonically
/// per grid and never reused, so a placement anchored to one can never be
/// re-pointed at later content on the same grid.
///
/// Nothing orders the ring by id. A reverse scroll inserts a freshly
/// minted row above rows minted earlier, and two consequences follow that
/// each invalidate a short-circuit a reader would otherwise reach for.
///
/// History becomes unordered as well, because such a row later scrolls
/// into it like any other, so binary-searching the history segment alone
/// is equally unsound. The front row is not necessarily the lowest id
/// either, because on a grid built without history a reverse scroll
/// inserts at ring index zero. The only sound constant-time rejection is
/// `id >= next_line_id`, which no caller can produce.
///
/// The uniqueness is per grid, NOT per terminal: the primary and
/// alternate screens own separate grids that both start at zero, so
/// resolving an id against the wrong one silently names a different row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    rows: VecDeque<StoredRow>,
    /// Active-screen dimensions; `rows` always keeps at least this
    /// many entries as its tail window.
    size: GridSize,
    /// History row cap: `history_len` never exceeds it, and a scroll
    /// at the cap recycles the evicted row as the incoming blank.
    max_history: usize,
    /// The id the next row to enter the ring will carry.
    next_line_id: u64,
}

/// One stored row: its identity together with its cells.
///
/// The id lives here rather than on [`Row`] because `Row<Cell>` is the
/// storage row while `Row<Run>` is the emitted one, so a field on `Row`
/// would carry grid identity into the frame's wire type.
#[derive(Debug)]
struct StoredRow {
    id: LineId,
    cells: Row<Cell>,
}

impl Grid {
    /// Builds a grid of blank visible rows with an empty history.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        let mut rows = VecDeque::with_capacity(usize::from(size.rows));
        for id in 0..u64::from(size.rows) {
            rows.push_back(StoredRow {
                id: LineId(id),
                cells: Row::filled(size.cols, Cell::default()),
            });
        }
        Self {
            rows,
            size,
            max_history,
            next_line_id: u64::from(size.rows),
        }
    }

    /// Grid dimensions in cells.
    pub const fn size(&self) -> GridSize {
        self.size
    }

    /// Overwrites the given column range of one visible row with `fill`.
    pub fn fill_visible_row_range(&mut self, line: ScreenLine, columns: Range<u16>, fill: Cell) {
        let index = self.visible_index(line.0);
        // NOTE: `Row`'s own `Index<u16>` shadows the slice's range
        // indexing, so the row has to reach the slice through `DerefMut`
        // before a range can be applied.
        let row: &mut [Cell] = &mut self.rows[index].cells;
        row[usize::from(columns.start)..usize::from(columns.end)].fill(fill);
    }

    /// Scrolls the visible screen up by one row: the top visible row
    /// becomes the newest history row and a `fill`-filled row enters at
    /// the bottom.
    pub(super) fn scroll_up_one(&mut self, fill: Cell) {
        let id = self.mint();
        if self.history_len() < self.max_history {
            self.rows.push_back(StoredRow {
                id,
                cells: Row::filled(self.size.cols, fill),
            });
            return;
        }
        let mut recycled = self
            .rows
            .pop_front()
            .expect("the ring always holds the visible rows");
        recycled.id = id;
        recycled.cells.fill(fill);
        self.rows.push_back(recycled);
    }

    /// Scrolls the region down by one row: a `fill`-filled row enters at
    /// `top` and the row at `bottom` is discarded.
    ///
    /// Deliberately not the inverse of [`Self::scroll_up_one`]. The row
    /// leaving the bottom is lost rather than becoming history, and the
    /// row entering at the top is blank rather than the newest history
    /// row; history is neither grown, trimmed, nor read back from.
    ///
    /// # Invariants
    ///
    /// The region satisfies `top <= bottom < size.rows`. Under that,
    /// `base + bottom` is at most the ring's last index and the later
    /// insert at `base + top` is in range, which is why neither
    /// `VecDeque` call can fail.
    pub(super) fn scroll_down_one(&mut self, top: ScreenLine, bottom: ScreenLine, fill: Cell) {
        debug_assert!(top <= bottom, "a scroll region runs top to bottom");
        debug_assert!(
            bottom.0 < self.size.rows,
            "the region's bottom row is on screen"
        );
        // NOTE: `history_len` is `rows.len() - size.rows`, so reading it
        // from the shortened ring underflows on a grid with no history —
        // which is how the alternate screen is built. It has to be bound
        // before the removal.
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
    pub(super) fn line_id(&self, line: ScreenLine) -> LineId {
        self.rows[self.visible_index(line.0)].id
    }

    /// The active-grid line the row `id` now sits at; `None` once it has
    /// left the ring.
    pub(super) fn grid_line(&self, id: LineId) -> Option<GridLine> {
        // NOTE: the ring is not ordered by id, so this scan must not be
        // turned into a binary search and must not be short-circuited on
        // the front row's id — see the invariants on `LineId`.
        let index = self.rows.iter().rposition(|row| row.id == id)?;
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
        &self.rows[index].cells
    }

    /// Number of history rows currently retained.
    pub fn history_len(&self) -> usize {
        self.rows.len() - usize::from(self.size.rows)
    }

    /// Hands out the next unused row id.
    ///
    /// # Invariants
    ///
    /// Ids only ever move forward, which is what makes them unique for
    /// the grid's lifetime; the overflow guard is what keeps that true.
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
}

/// Indexes the row at a screen line; history rows are structurally
/// unreachable because [`ScreenLine`] cannot be negative.
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
        grid.fill_visible_row_range(ScreenLine(0), 1..3, Cell::default());
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

    mod scroll_down {
        use super::*;

        /// A grid whose visible rows are labelled `a`, `b`, `c`, … in
        /// column zero, so a scroll is legible in the row contents.
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

        /// Asserts that a reverse scroll leaves the history untouched.
        ///
        /// The agreed policy discards the row that falls off the bottom
        /// rather than pushing it into history, and fills the incoming top
        /// row blank rather than pulling the newest history row back. A
        /// reverse scroll is deliberately not the inverse of a forward
        /// one: scrollback records what the terminal has emitted, and a
        /// reverse scroll emits nothing.
        ///
        /// Case: a full-screen editor scrolls its view backwards while the
        /// shell's earlier output still sits in scrollback behind it.
        #[test]
        fn a_reverse_scroll_leaves_history_alone() {
            let mut grid = labelled(3, 10);
            grid.scroll_up_one(Cell::default());
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
        /// backwards, expecting the exposed row to carry that colour
        /// rather than the terminal default.
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
    }
}
