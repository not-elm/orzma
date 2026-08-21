//! The atomic grid + cursor operation unit for one terminal screen.
//!
//! [`Screen`] owns cell storage ([`grid::Grid`]) and the write cursor,
//! and updates them together; every mutation returns the [`Damage`] it
//! produced for the caller to stage instead of staging internally.

pub mod cell;
pub mod cursor;
pub mod grid;
pub mod margins;
mod state;
pub mod viewport;

use self::cell::{Cell, Pen};
use self::grid::Grid;
use self::grid::LineId;
use self::grid::row::Row;
use crate::damage::Damage;
use crate::schema::{
    Cursor, CursorShape, DisplayOffset, GridColumn, GridLine, GridPoint, GridSize, ScreenLine,
    ViewportLine,
};
use crate::screen::cursor::SavedCursorSlots;
use crate::screen::margins::Margins;
use crate::screen::state::ScreenState;
use crate::screen::viewport::Viewport;

/// Span selector for [`Screen::erase_in_line`] (`CSI K`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseLineMode {
    /// From the cursor to the end of the row (`EL 0`).
    ToEnd,
    /// From the start of the row through the cursor column (`EL 1`).
    ToStart,
    /// The whole row (`EL 2`).
    All,
}

/// Span selector for [`Screen::erase_in_display`] (`CSI J`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseScreenMode {
    /// From the cursor cell to the end of the screen (`ED 0`).
    Below,
    /// From the top of the screen through the cursor cell (`ED 1`).
    Above,
    /// The whole visible screen (`ED 2`); history is untouched.
    All,
}

/// One terminal screen: cell storage plus the write cursor, updated
/// atomically by each operation.
///
/// # Invariants
///
/// Both grid axes are nonzero; degenerate sizes are rejected by the
/// caller (the same contract as [`crate::Vt::resize`]).
pub struct Screen {
    grid: Grid,
    viewport: Viewport,
    state: ScreenState,
    #[expect(
        dead_code,
        reason = "DECSC/DECRC arrive in a later step of the implementation order"
    )]
    saved: SavedCursorSlots,
    margins: Margins,
}

impl Screen {
    /// Builds a blank screen with the cursor at the origin and the
    /// viewport pinned to the live tail.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            grid: Grid::new(size, max_history),
            viewport: Viewport::default(),
            state: ScreenState::default(),
            saved: SavedCursorSlots::default(),
            margins: Margins::new(size.rows),
        }
    }

    /// Prints one character at the cursor with the current pen,
    /// wrapping first when the deferred wrap is armed.
    ///
    /// The caller dispatches control bytes itself; this method assumes
    /// a printable character of display width one.
    pub fn print(&mut self, c: char) -> Option<Damage> {
        let damage = if self.state.pending_wrap {
            self.state.pending_wrap = false;
            self.state.column = GridColumn(0);
            self.lf()
        } else {
            Some(self.damage_span(self.state.line, self.state.line))
        };
        self.grid[self.state.line][self.state.column] = self.state.pen.stamp(c);
        if self.state.column.0 + 1 < self.grid.size().cols {
            self.state.column.0 += 1;
        } else {
            self.state.pending_wrap = true;
        }
        damage
    }

    /// Rewinds the cursor to column zero and disarms the deferred wrap.
    pub fn cr(&mut self) -> Option<Damage> {
        self.state.column = GridColumn(0);
        self.state.pending_wrap = false;
        Some(self.damage_span(self.state.line, self.state.line))
    }

    /// Moves the cursor down one row, scrolling at the bottom margin;
    /// the deferred-wrap flag is deliberately preserved.
    pub fn lf(&mut self) -> Option<Damage> {
        if self.state.line < self.margins.bottom {
            let departed = self.state.line;
            self.state.line.0 += 1;
            return Some(self.damage_span(departed, self.state.line));
        }
        self.grid.scroll_up_one(self.state.pen.erase_cell());
        self.hold_scrolled_viewport();
        Some(Damage::Full)
    }

    /// Erases part of the cursor row with the pen background (BCE);
    /// [`EraseLineMode::ToEnd`] is a no-op while the deferred wrap is
    /// armed.
    pub fn erase_in_line(&mut self, mode: EraseLineMode) -> Option<Damage> {
        if matches!(mode, EraseLineMode::ToEnd) && self.state.pending_wrap {
            return None;
        }
        let cols = self.grid.size().cols;
        let columns = match mode {
            EraseLineMode::ToEnd => self.state.column.0..cols,
            EraseLineMode::ToStart => 0..self.state.column.0 + 1,
            EraseLineMode::All => 0..cols,
        };
        self.grid
            .fill_visible_row_range(self.state.line.0, columns, self.state.pen.erase_cell());
        Some(self.damage_span(self.state.line, self.state.line))
    }

    /// Erases part of the visible screen with the pen background
    /// (BCE), in place; scrollback history is never touched.
    pub fn erase_in_display(&mut self, mode: EraseScreenMode) -> Option<Damage> {
        let GridSize { cols, rows } = self.grid.size();
        let blank = self.state.pen.erase_cell();
        match mode {
            EraseScreenMode::Below => {
                self.grid.fill_visible_row_range(
                    self.state.line.0,
                    self.state.column.0..cols,
                    blank,
                );
                for line in self.state.line.0 + 1..rows {
                    self.grid.fill_visible_row_range(line, 0..cols, blank);
                }
                Some(self.damage_span(self.state.line, ScreenLine(rows - 1)))
            }
            EraseScreenMode::Above => {
                for line in 0..self.state.line.0 {
                    self.grid.fill_visible_row_range(line, 0..cols, blank);
                }
                self.grid.fill_visible_row_range(
                    self.state.line.0,
                    0..self.state.column.0 + 1,
                    blank,
                );
                Some(self.damage_span(ScreenLine(0), self.state.line))
            }
            EraseScreenMode::All => {
                for line in 0..rows {
                    self.grid.fill_visible_row_range(line, 0..cols, blank);
                }
                Some(Damage::Full)
            }
        }
    }

    /// Follows a one-row scroll with the offset that keeps a scrolled
    /// viewport on the content it was showing.
    ///
    /// A viewport pinned to the live tail stays pinned — that is what
    /// following the newest output means. A scrolled one counts one row
    /// further back, because the row it shows just moved that far from
    /// the tail.
    ///
    /// # Invariants
    ///
    /// The offset is clamped to the history that survives the scroll.
    /// At capacity the row the user was reading has been evicted, so
    /// the view drifts by one; there is nothing left to hold on.
    fn hold_scrolled_viewport(&mut self) {
        if self.viewport.offset == DisplayOffset(0) {
            return;
        }
        let history =
            u32::try_from(self.grid.history_len()).expect("scrollback never exceeds u32::MAX rows");
        self.viewport.offset = DisplayOffset(self.viewport.offset.0.saturating_add(1).min(history));
    }

    /// Reports the given screen rows as damage, in the viewport
    /// coordinates a frame repaints by.
    ///
    /// Rows the viewport does not show are not repainted, so a span that
    /// starts past the last visible row becomes [`Damage::Metadata`]: the
    /// frame is still emitted, it just carries no dirty rows.
    fn damage_span(&self, first: ScreenLine, last: ScreenLine) -> Damage {
        debug_assert!(first <= last, "a damage span runs top to bottom");
        let rows = self.grid.size().rows;
        // NOTE: `DisplayOffset` is a `u32` and does not bound itself, so the
        // shift has to saturate — a wrapping add would report an off-screen
        // row as visible.
        let offset = self.viewport.offset.0;
        let first = u32::from(first.0).saturating_add(offset);
        if first >= u32::from(rows) {
            return Damage::Metadata;
        }
        let last = u32::from(last.0)
            .saturating_add(offset)
            .min(u32::from(rows - 1));
        Damage::rows(
            ViewportLine(u16::try_from(first).expect("guarded above by first < rows")),
            ViewportLine(u16::try_from(last).expect("clamped to rows - 1 above")),
        )
    }

    /// Returns the grid size.
    pub fn grid_size(&self) -> GridSize {
        self.grid.size()
    }

    /// Borrows the cells shown at a viewport line.
    ///
    /// The viewport is the window the user sees: at the live tail it is
    /// the active screen, and a scrolled viewport reaches back into
    /// history. [`crate::screen::grid::Grid`]'s own index resolves
    /// against the live tail alone, so a scrolled read has to come
    /// through here.
    pub(crate) fn viewport_row(&self, line: ViewportLine) -> &Row<Cell> {
        let offset =
            i32::try_from(self.viewport.offset.0).expect("scrollback never exceeds i32::MAX rows");
        self.grid.row(GridLine(i32::from(line.0) - offset))
    }

    /// The write cursor as an emitted frame carries it.
    // TODO: Report the real shape, blink, and visibility once DECSCUSR
    // and DECTCEM land. Block / steady / visible is what the terminal
    // starts at.
    pub(crate) fn cursor(&self) -> Cursor {
        Cursor {
            point: GridPoint {
                line: GridLine::from(self.state.line),
                column: self.state.column,
            },
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    }

    /// Mutably borrows the SGR pen; applying SGR sequences is the
    /// caller's job.
    pub fn pen_mut(&mut self) -> &mut Pen {
        &mut self.state.pen
    }

    /// Number of scrollback rows the viewport sits above the live tail; always zero until scroll operations arrive.
    #[inline]
    pub const fn display_offset(&self) -> DisplayOffset {
        self.viewport.offset
    }

    /// Moves the viewport to `offset`.
    ///
    /// The caller clamps to the history that exists; this is the only seam
    /// that writes the offset.
    pub(crate) fn set_display_offset(&mut self, offset: DisplayOffset) {
        self.viewport.offset = offset;
    }

    /// The id of the row the cursor sits on — the anchor a mount samples.
    pub(crate) fn cursor_line_id(&self) -> LineId {
        self.grid.line_id(self.state.line)
    }

    /// The signed viewport row `id`'s row now sits at; `None` once the row
    /// has left the ring.
    ///
    /// Unlike damage projection this does not cull: a placement whose
    /// anchor sits above the viewport reports a negative row and the
    /// renderer clips it. The value saturates rather than reusing `None`,
    /// which already means the row is gone.
    pub(crate) fn viewport_row_of(&self, id: LineId) -> Option<i32> {
        let line = self.grid.grid_line(id)?;
        let row = i64::from(line.0) + i64::from(self.viewport.offset.0);
        Some(row.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
    }

    /// The cursor's column.
    pub(crate) fn cursor_column(&self) -> GridColumn {
        self.state.column
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Color;

    fn screen() -> Screen {
        Screen::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Asserts that a viewport row at the live tail is the visible row
    /// with the same index.
    ///
    /// Case: the emitter builds a snapshot for a terminal the user has
    /// not scrolled.
    #[test]
    fn a_viewport_row_at_the_live_tail_is_the_visible_row() {
        let mut screen = screen();
        screen.grid[ScreenLine(1)][0].c = 'x';
        assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, 'x');
    }

    /// Asserts that a scrolled viewport reads the history rows it
    /// shows rather than the live tail.
    ///
    /// Case: the user scrolls back one line, so the top of the window
    /// is the newest scrollback row and the live rows shift down.
    #[test]
    fn a_scrolled_viewport_row_reads_history() {
        let mut screen = screen();
        screen.grid[ScreenLine(0)][0].c = 'a';
        screen.state.line = ScreenLine(2);
        screen.lf();
        assert_eq!(screen.grid.history_len(), 1);
        screen.viewport.offset = DisplayOffset(1);
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    }

    /// Asserts that the reported cursor carries the write position and
    /// is visible.
    ///
    /// The agreed placeholder is Block / steady / visible, matching what
    /// a terminal starts at; `Cursor::default()` is deliberately not
    /// used because its `visible` is `false`, which would hide the
    /// caret until the first DECTCEM.
    ///
    /// Case: a shell prints its prompt and the next frame has to show
    /// the caret after it.
    #[test]
    fn the_cursor_reports_the_write_position_and_is_visible() {
        let mut screen = screen();
        screen.print('a');
        screen.print('b');
        let cursor = screen.cursor();
        assert_eq!(cursor.point.line, GridLine(0));
        assert_eq!(cursor.point.column, GridColumn(2));
        assert_eq!(cursor.shape, CursorShape::Block);
        assert!(!cursor.blinking);
        assert!(cursor.visible);
    }

    /// Asserts that output arriving while the user is scrolled back
    /// leaves the viewed content where it was.
    ///
    /// The agreed policy holds the viewport still rather than letting
    /// it drift with the live tail: `VtBackend::scroll` already pins
    /// "the viewport holds its position while the PTY emits output",
    /// and every terminal that keeps scrollback behaves this way.
    ///
    /// Case: the user is reading an earlier command's output when a
    /// background build prints its next line.
    #[test]
    fn output_below_a_scrolled_viewport_holds_the_view_still() {
        let mut screen = screen();
        screen.grid[ScreenLine(0)][0].c = 'a';
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.viewport.offset = DisplayOffset(1);
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');

        screen.state.line = ScreenLine(2);
        screen.lf();
        assert_eq!(screen.display_offset(), DisplayOffset(2));
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    }

    /// Asserts that output at the live tail leaves the viewport pinned
    /// there.
    ///
    /// Case: an unscrolled terminal keeps printing, and the window has
    /// to follow the newest line rather than freeze.
    #[test]
    fn output_at_the_live_tail_keeps_the_viewport_pinned() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.lf();
        assert_eq!(screen.display_offset(), DisplayOffset(0));
    }

    /// Asserts that a scroll at history capacity clamps the offset
    /// instead of naming a row the ring no longer holds.
    ///
    /// The agreed policy accepts that the view drifts once scrollback
    /// is full: the row the user was reading has been evicted, so there
    /// is nothing left to hold still on.
    ///
    /// Case: the user is parked at the top of a full scrollback while
    /// output keeps arriving.
    #[test]
    fn a_scroll_at_history_capacity_clamps_the_offset() {
        let mut screen = Screen::new(GridSize { cols: 4, rows: 3 }, 1);
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.viewport.offset = DisplayOffset(1);
        screen.state.line = ScreenLine(2);
        screen.lf();
        assert_eq!(screen.grid.history_len(), 1);
        assert_eq!(screen.display_offset(), DisplayOffset(1));
    }

    /// Asserts that damage is reported in viewport rows, not the grid rows
    /// the write used.
    ///
    /// Case: the user scrolls back one row and the shell echoes a character
    /// at the live tail, which now sits one row lower in the window.
    #[test]
    fn a_scrolled_screen_reports_damage_in_viewport_rows() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.set_display_offset(DisplayOffset(1));
        screen.state.line = ScreenLine(0);
        assert_eq!(
            screen.cr(),
            Some(Damage::rows(ViewportLine(1), ViewportLine(1)))
        );
    }

    /// Asserts that a write whose rows all sit below the viewport still
    /// forces a frame, carrying no dirty rows.
    ///
    /// Dropping the damage entirely would suppress the emit, which the
    /// ledger's own contract forbids — the frame still carries the current
    /// cursor and offset.
    ///
    /// Case: the viewport sits fully scrolled back into history while the
    /// shell keeps writing at the live tail.
    #[test]
    fn a_write_below_the_viewport_reports_metadata_damage() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.set_display_offset(DisplayOffset(1));
        screen.state.line = ScreenLine(2);
        assert_eq!(screen.cr(), Some(Damage::Metadata));
    }

    /// Asserts that a write below the bottom of the scrolled window
    /// reports no dirty row.
    ///
    /// The agreed policy reports [`Damage::Metadata`] rather than
    /// dropping the write outright: a frame still has to carry the
    /// current cursor and offset even though the row the write landed
    /// on has scrolled out of the window.
    ///
    /// Case: the user reads scrollback while a build keeps printing at
    /// the live tail, which the window no longer shows.
    #[test]
    fn a_write_scrolled_out_of_the_window_reports_no_dirty_row() {
        let mut screen = screen();
        for _ in 0..3 {
            screen.state.line = ScreenLine(2);
            screen.lf();
        }
        screen.viewport.offset = DisplayOffset(3);
        screen.state.line = ScreenLine(0);
        assert_eq!(screen.print('x'), Some(Damage::Metadata));
    }

    /// Asserts that an erase spanning the screen reports only the rows
    /// the scrolled window still shows.
    ///
    /// Case: a full-screen application clears from the cursor down
    /// while the user is scrolled back, so the lower part of the erased
    /// span has already left the window.
    #[test]
    fn a_scrolled_erase_reports_only_the_rows_still_in_the_window() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.viewport.offset = DisplayOffset(1);
        screen.state.line = ScreenLine(0);
        assert_eq!(
            screen.erase_in_display(EraseScreenMode::Below),
            Some(Damage::rows(ViewportLine(1), ViewportLine(2)))
        );
    }

    /// Asserts that a span reaching past the last visible row is clamped
    /// rather than reported out of range.
    ///
    /// Case: the user scrolls back and the application erases from the
    /// cursor to the bottom of the screen.
    #[test]
    fn a_span_running_past_the_viewport_is_clamped_to_its_last_row() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.set_display_offset(DisplayOffset(1));
        screen.state.line = ScreenLine(0);
        assert_eq!(
            screen.erase_in_display(EraseScreenMode::Below),
            Some(Damage::rows(ViewportLine(1), ViewportLine(2)))
        );
    }

    /// Asserts that a fresh screen starts at the origin, pinned to the
    /// live tail, with an empty history.
    ///
    /// Case: a terminal spawns and the first shell output must land at
    /// the top-left of an unscrolled screen.
    #[test]
    fn a_fresh_screen_starts_at_the_origin() {
        let screen = screen();
        assert_eq!(
            (screen.state.line, screen.state.column),
            (ScreenLine(0), GridColumn(0))
        );
        assert_eq!(screen.display_offset(), DisplayOffset(0));
        assert_eq!(screen.grid.history_len(), 0);
    }

    /// Asserts that a carriage return rewinds the column and clears the
    /// deferred-wrap flag, damaging the cursor row.
    ///
    /// Case: a shell prints a partial line and returns to overwrite it,
    /// as progress indicators do with a bare `\r`.
    #[test]
    fn carriage_return_rewinds_and_clears_pending_wrap() {
        let mut screen = screen();
        screen.state.column = GridColumn(2);
        screen.state.pending_wrap = true;
        let damage = screen.cr();
        assert_eq!(screen.state.column, GridColumn(0));
        assert!(!screen.state.pending_wrap);
        assert_eq!(damage, Some(Damage::rows(ViewportLine(0), ViewportLine(0))));
    }

    /// Asserts that a linefeed above the bottom row only moves the
    /// cursor, damaging the departed and arrived rows.
    ///
    /// Case: a shell prints multiple output lines while the screen
    /// still has empty rows below the cursor.
    #[test]
    fn a_linefeed_above_the_bottom_moves_the_cursor() {
        let mut screen = screen();
        let damage = screen.lf();
        assert_eq!(screen.state.line, ScreenLine(1));
        assert_eq!(damage, Some(Damage::rows(ViewportLine(0), ViewportLine(1))));
    }

    /// Asserts that a linefeed at the bottom margin scrolls the screen and
    /// pushes the departing row into history.
    ///
    /// Case: a shell prints past the last row and the earlier output has to
    /// remain reachable by scrolling back.
    #[test]
    fn a_bottom_linefeed_scrolls_and_pushes_history() {
        let mut screen = screen();
        screen.grid[ScreenLine(0)][GridColumn(0)].c = 'a';
        screen.state.line = ScreenLine(2);
        assert_eq!(screen.lf(), Some(Damage::Full));
        assert_eq!(screen.grid.history_len(), 1);
    }

    /// Asserts that the row scrolled in at the bottom carries the
    /// current pen background.
    ///
    /// Case: an application sets a colored background and scrolls at
    /// the bottom of the screen.
    #[test]
    fn a_scrolled_in_row_carries_the_pen_background() {
        let mut screen = screen();
        screen.pen_mut().bg = Color::Indexed(4);
        screen.state.line = ScreenLine(2);
        screen.lf();
        assert_eq!(screen.grid[ScreenLine(2)][0].bg, Color::Indexed(4));
        assert_eq!(screen.grid[ScreenLine(2)][3].bg, Color::Indexed(4));
    }

    /// Asserts that a linefeed preserves the deferred-wrap flag.
    ///
    /// The agreed policy follows alacritty: only a carriage return or
    /// an explicit cursor motion clears the pending wrap; a bare
    /// linefeed does not.
    ///
    /// Case: an application writes a full-width line, then emits a bare
    /// linefeed before continuing to print on the next row.
    #[test]
    fn a_linefeed_preserves_pending_wrap() {
        let mut screen = screen();
        screen.state.pending_wrap = true;
        screen.lf();
        assert!(screen.state.pending_wrap);
    }

    /// Asserts that printing stamps the pen into the cell and advances
    /// the cursor one column.
    ///
    /// Case: an application prints ordinary colored text at the start
    /// of a row.
    #[test]
    fn print_stamps_the_pen_and_advances() {
        let mut screen = screen();
        screen.pen_mut().fg = Color::Indexed(1);
        let damage = screen.print('a');
        assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
        assert_eq!(screen.grid[ScreenLine(0)][0].fg, Color::Indexed(1));
        assert_eq!(
            (screen.state.line, screen.state.column),
            (ScreenLine(0), GridColumn(1))
        );
        assert_eq!(damage, Some(Damage::rows(ViewportLine(0), ViewportLine(0))));
    }

    /// Asserts that printing into the last column arms the deferred
    /// wrap and leaves the cursor in place.
    ///
    /// Case: an application emits a line exactly as wide as the
    /// screen, and the terminal must not move to the next row until
    /// more text actually arrives.
    #[test]
    fn print_at_the_last_column_arms_the_deferred_wrap() {
        let mut screen = screen();
        screen.state.column = GridColumn(3);
        screen.print('x');
        assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
        assert_eq!(screen.state.column, GridColumn(3));
        assert!(screen.state.pending_wrap);
    }

    /// Asserts that the print following an armed deferred wrap lands
    /// at the start of the next row.
    ///
    /// Case: an application prints past the right edge, and the
    /// overflowing character continues on the next line.
    #[test]
    fn the_next_print_after_the_last_column_wraps() {
        let mut screen = screen();
        for c in ['a', 'b', 'c', 'd'] {
            screen.print(c);
        }
        let damage = screen.print('e');
        assert_eq!(screen.grid[ScreenLine(1)][0].c, 'e');
        assert_eq!(
            (screen.state.line, screen.state.column),
            (ScreenLine(1), GridColumn(1))
        );
        assert!(!screen.state.pending_wrap);
        assert_eq!(damage, Some(Damage::rows(ViewportLine(0), ViewportLine(1))));
    }

    /// Asserts that a deferred wrap on the bottom row scrolls the
    /// screen and reports full damage.
    ///
    /// Case: a shell fills the very last cell of the screen and keeps
    /// printing, forcing a scroll in the middle of the wrap.
    #[test]
    fn a_wrap_on_the_bottom_row_scrolls() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.state.column = GridColumn(3);
        screen.print('x');
        let damage = screen.print('y');
        assert_eq!(screen.grid[ScreenLine(2)][0].c, 'y');
        assert_eq!(damage, Some(Damage::Full));
    }

    /// Asserts that erase-to-end clears from the cursor to the right
    /// edge with the pen background.
    ///
    /// Case: an application with a colored background truncates the
    /// tail of a line with `EL 0`.
    #[test]
    fn erase_to_end_clears_from_the_cursor_with_the_pen_background() {
        let mut screen = screen();
        for c in ['a', 'b', 'c'] {
            screen.print(c);
        }
        screen.state.column = GridColumn(1);
        screen.pen_mut().bg = Color::Indexed(2);
        let damage = screen.erase_in_line(EraseLineMode::ToEnd);
        assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
        assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
        assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(2));
        assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(2));
        assert_eq!(damage, Some(Damage::rows(ViewportLine(0), ViewportLine(0))));
    }

    /// Asserts that erase-to-start clears through the cursor column
    /// inclusively.
    ///
    /// The agreed convention matches `EL 1`: the erased span is
    /// `0..=cursor.column`, the classic off-by-one of this operation.
    ///
    /// Case: an application rewrites the head of a line and clears
    /// what it had written so far, cursor included.
    #[test]
    fn erase_to_start_includes_the_cursor_column() {
        let mut screen = screen();
        for c in ['a', 'b', 'c'] {
            screen.print(c);
        }
        screen.state.column = GridColumn(1);
        screen.erase_in_line(EraseLineMode::ToStart);
        assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
        assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
        assert_eq!(screen.grid[ScreenLine(0)][2].c, 'c');
    }

    /// Asserts that erase-to-end is a no-op while the deferred wrap is
    /// armed.
    ///
    /// The agreed policy follows alacritty: with the wrap pending the
    /// cursor logically sits past the row's last cell, so `EL 0`
    /// erases nothing rather than the just-printed last cell.
    ///
    /// Case: an application fills a row to its last column and then
    /// issues `EL 0` before printing anything further.
    #[test]
    fn erase_to_end_is_a_no_op_under_pending_wrap() {
        let mut screen = screen();
        for c in ['a', 'b', 'c', 'd'] {
            screen.print(c);
        }
        let damage = screen.erase_in_line(EraseLineMode::ToEnd);
        assert_eq!(screen.grid[ScreenLine(0)][3].c, 'd');
        assert_eq!(damage, None);
    }

    /// Asserts that erase-all clears the whole row regardless of the
    /// cursor column.
    ///
    /// Case: a full-screen application repaints a status line in place
    /// by clearing the entire row with `EL 2` before rewriting it.
    #[test]
    fn erase_all_clears_the_whole_row() {
        let mut screen = screen();
        for c in ['a', 'b', 'c'] {
            screen.print(c);
        }
        screen.state.column = GridColumn(1);
        screen.erase_in_line(EraseLineMode::All);
        assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
        assert_eq!(screen.grid[ScreenLine(0)][2].c, ' ');
    }

    /// Asserts that erase-below clears from the cursor cell to the end
    /// of the screen, leaving earlier content in place.
    ///
    /// Case: a full-screen application redraws everything under the
    /// cursor with `ED 0` while the rows above stay intact.
    #[test]
    fn erase_display_below_clears_from_the_cursor_down() {
        let mut screen = screen();
        screen.print('a');
        screen.lf();
        screen.cr();
        for c in ['b', 'c'] {
            screen.print(c);
        }
        screen.state.column = GridColumn(1);
        let damage = screen.erase_in_display(EraseScreenMode::Below);
        assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
        assert_eq!(screen.grid[ScreenLine(1)][0].c, 'b');
        assert_eq!(screen.grid[ScreenLine(1)][1].c, ' ');
        assert_eq!(damage, Some(Damage::rows(ViewportLine(1), ViewportLine(2))));
    }

    /// Asserts that erase-above clears everything through the cursor
    /// cell inclusively, leaving the rest of the cursor row intact.
    ///
    /// Case: a full-screen application discards everything already
    /// drawn above and left of the cursor with `ED 1`.
    #[test]
    fn erase_display_above_clears_through_the_cursor() {
        let mut screen = screen();
        screen.print('a');
        screen.lf();
        screen.cr();
        for c in ['b', 'c', 'd'] {
            screen.print(c);
        }
        screen.state.column = GridColumn(1);
        let damage = screen.erase_in_display(EraseScreenMode::Above);
        assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
        assert_eq!(screen.grid[ScreenLine(1)][0].c, ' ');
        assert_eq!(screen.grid[ScreenLine(1)][1].c, ' ');
        assert_eq!(screen.grid[ScreenLine(1)][2].c, 'd');
        assert_eq!(damage, Some(Damage::rows(ViewportLine(0), ViewportLine(1))));
    }

    /// Asserts that erase-all clears the visible screen in place while
    /// scrollback history survives.
    ///
    /// The agreed policy is the classic xterm behavior: `ED 2` erases
    /// in place and does not push the cleared rows into history (a
    /// deliberate divergence from alacritty, which scrolls them out
    /// first).
    ///
    /// Case: the user runs `clear` in a session that already
    /// accumulated scrollback, then scrolls back to check older
    /// output.
    #[test]
    fn erase_display_all_clears_the_screen_but_not_history() {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.cr();
        for c in ['a', 'b'] {
            screen.print(c);
        }
        let damage = screen.erase_in_display(EraseScreenMode::All);
        assert_eq!(screen.grid[ScreenLine(2)][0].c, ' ');
        assert_eq!(screen.grid[ScreenLine(2)][1].c, ' ');
        assert_eq!(screen.grid.history_len(), 1);
        assert_eq!(damage, Some(Damage::Full));
    }

    /// Asserts that an anchor above the viewport reports a negative row
    /// rather than being dropped.
    ///
    /// Case: a mounted webview scrolls off the top of the window while the
    /// user keeps working below it.
    #[test]
    fn an_anchor_above_the_viewport_reports_a_negative_row() {
        let mut screen = screen();
        let id = screen.cursor_line_id();
        assert_eq!(screen.viewport_row_of(id), Some(0));
        screen.state.line = ScreenLine(2);
        screen.lf();
        assert_eq!(screen.viewport_row_of(id), Some(-1));
    }

    /// Asserts that scrolling the viewport back moves an anchor's reported
    /// row down by the same amount.
    ///
    /// Case: the user scrolls up to re-read output, and a webview anchored
    /// in that output must be painted where its text now sits.
    #[test]
    fn scrolling_back_moves_an_anchors_reported_row_down() {
        let mut screen = screen();
        let id = screen.cursor_line_id();
        screen.state.line = ScreenLine(2);
        screen.lf();
        screen.set_display_offset(DisplayOffset(1));
        assert_eq!(screen.viewport_row_of(id), Some(0));
    }

    /// Asserts that an anchor whose row left the ring stops resolving.
    ///
    /// Case: the scrollback fills and the row a webview was anchored to is
    /// finally trimmed away.
    #[test]
    fn an_anchor_trimmed_from_the_ring_stops_resolving() {
        let mut screen = Screen::new(GridSize { cols: 4, rows: 3 }, 1);
        let id = screen.cursor_line_id();
        for _ in 0..2 {
            screen.state.line = ScreenLine(2);
            screen.lf();
        }
        assert_eq!(screen.viewport_row_of(id), None);
    }
}
