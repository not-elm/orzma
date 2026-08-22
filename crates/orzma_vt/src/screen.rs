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
mod tabs;
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
use crate::screen::tabs::{CharacterTabEdit, TabStops};
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
    tabs: TabStops,
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
            tabs: TabStops::default(),
        }
    }

    /// Prints one character at the cursor with the current pen,
    /// wrapping first when the deferred wrap is armed.
    ///
    /// The caller dispatches control bytes itself; this method assumes
    /// a printable character of display width one.
    ///
    /// The reported damage always covers the row the character landed
    /// on: a wrap that scrolled reports [`Damage::Full`], and every
    /// other print reports its own row. [`Self::lf`] reports the wrap's
    /// cursor motion alone, so passing that value through would leave
    /// the character just written unpainted.
    pub fn print(&mut self, c: char) -> Option<Damage> {
        let wrap = if self.state.pending_wrap {
            self.state.pending_wrap = false;
            self.state.column = GridColumn(0);
            self.lf()
        } else {
            None
        };
        self.grid[self.state.line][self.state.column] = self.state.pen.stamp(c);
        if self.state.column.0 + 1 < self.grid.size().cols {
            self.state.column.0 += 1;
        } else {
            self.state.pending_wrap = true;
        }
        match wrap {
            Some(Damage::Full) => Some(Damage::Full),
            _ => Some(self.damage_span(self.state.line, self.state.line)),
        }
    }

    /// Moves the cursor one column left and disarms the deferred wrap.
    ///
    /// The cursor stops at column zero rather than wrapping back onto
    /// the previous row: xterm reaches that row only under
    /// reverse-wraparound (`DECSET 45` / `DECSET 1045`), which is off by
    /// default and unimplemented here, and it additionally requires
    /// autowrap to be on.
    ///
    /// Reports no damage when the cursor already sits at column zero
    /// with the wrap disarmed, on the same reasoning as [`Screen::cr`].
    ///
    /// # Invariants
    ///
    /// The deferred wrap is disarmed even when the column does not
    /// change, and the column steps back even when the wrap was armed.
    /// xterm's `CursorBack` decrements unconditionally without
    /// reverse-wraparound and ends in `ResetWrap`, so a backspace after
    /// a full row lands one column short of the cell just written, not
    /// on it.
    pub fn bs(&mut self) -> Option<Damage> {
        if self.state.column == GridColumn(0) && !self.state.pending_wrap {
            return None;
        }
        self.state.column = GridColumn(self.state.column.0.saturating_sub(1));
        self.state.pending_wrap = false;
        Some(Damage::Metadata)
    }

    /// Rewinds the cursor to column zero and disarms the deferred wrap.
    ///
    /// Reports no damage when the cursor already sits at column zero
    /// with the wrap disarmed: the call writes no cell and moves
    /// nothing, so the frame it would force repeats the last one. A
    /// rewind that does move the cursor reports [`Damage::Metadata`],
    /// because no cell changed either way.
    pub fn cr(&mut self) -> Option<Damage> {
        if self.state.column == GridColumn(0) && !self.state.pending_wrap {
            return None;
        }
        self.state.column = GridColumn(0);
        self.state.pending_wrap = false;
        Some(Damage::Metadata)
    }

    /// Moves the cursor down one row, scrolling at the bottom margin;
    /// the deferred-wrap flag is deliberately preserved.
    ///
    /// A move inside the screen reports [`Damage::Metadata`]: neither
    /// the departed nor the arrived row changes contents, and the caret
    /// reaches the renderer through the frame's cursor. Scrolling moves
    /// content and reports [`Damage::Full`].
    pub fn lf(&mut self) -> Option<Damage> {
        if self.state.line < self.margins.bottom {
            self.state.line.0 += 1;
            return Some(Damage::Metadata);
        }
        self.grid.scroll_up_one(self.state.pen.erase_cell());
        self.hold_scrolled_viewport();
        Some(Damage::Full)
    }

    /// Moves the cursor up one row, scrolling the region at its top
    /// margin (RI).
    ///
    /// ECMA-48 § 6.1.7 leaves a movement past the first line undefined and
    /// lists seven permitted behaviours; this takes (f), scrolling, which
    /// is the DEC and xterm behaviour applications expect. Unlike
    /// [`Self::lf`], the deferred wrap is disarmed: RI is an explicit
    /// cursor movement, and xterm reaches its cursor-up helper — which
    /// resets the flag — on both paths.
    ///
    /// A cursor above a non-zero top margin and already on the first row
    /// moves nothing and scrolls nothing, which is why the disarmed wrap
    /// is the only thing left to report there.
    pub fn ri(&mut self) -> Option<Damage> {
        let was_armed = self.state.pending_wrap;
        self.state.pending_wrap = false;
        if self.state.line == self.margins.top {
            self.grid.scroll_down_one(
                self.margins.top,
                self.margins.bottom,
                self.state.pen.erase_cell(),
            );
            return Some(Damage::Full);
        }
        if ScreenLine(0) < self.state.line {
            self.state.line.0 -= 1;
            return Some(Damage::Metadata);
        }
        was_armed.then_some(Damage::Metadata)
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
            .fill_visible_row_range(self.state.line, columns, self.state.pen.erase_cell());
        Some(self.damage_span(self.state.line, self.state.line))
    }

    /// Erases part of the visible screen with the pen background
    /// (BCE), in place; scrollback history is never touched.
    pub fn erase_in_display(&mut self, mode: EraseScreenMode) -> Option<Damage> {
        let GridSize { cols, rows } = self.grid.size();
        let blank = self.state.pen.erase_cell();
        match mode {
            EraseScreenMode::Below => {
                self.grid
                    .fill_visible_row_range(self.state.line, self.state.column.0..cols, blank);
                for line in self.state.line.0 + 1..rows {
                    self.grid
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank);
                }
                Some(self.damage_span(self.state.line, ScreenLine(rows - 1)))
            }
            EraseScreenMode::Above => {
                for line in 0..self.state.line.0 {
                    self.grid
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank);
                }
                self.grid.fill_visible_row_range(
                    self.state.line,
                    0..self.state.column.0 + 1,
                    blank,
                );
                Some(self.damage_span(ScreenLine(0), self.state.line))
            }
            EraseScreenMode::All => {
                for line in 0..rows {
                    self.grid
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank);
                }
                Some(Damage::Full)
            }
        }
    }

    /// Moves the cursor to the first stop past it (HT).
    pub fn ht(&mut self) -> Option<Damage> {
        self.cht(1)
    }

    /// Moves the cursor forward `count` tabulation stops (CHT).
    ///
    /// The right edge is this screen's last column, so the same stop
    /// table lands the cursor differently on a narrow screen than on a
    /// wide one.
    pub fn cht(&mut self, count: u16) -> Option<Damage> {
        let right_edge = GridColumn(self.grid_size().cols - 1);
        let target = self.tabs.cht(self.state.column, count, right_edge);
        self.tab_to(target)
    }

    /// Moves the cursor back `count` tabulation stops (CBT).
    ///
    /// The left edge is column zero until DECSLRM and DECOM land, at
    /// which point the margin supplies it instead.
    pub fn cbt(&mut self, count: u16) -> Option<Damage> {
        let target = self.tabs.cbt(self.state.column, count, GridColumn(0));
        self.tab_to(target)
    }

    /// Sets a tabulation stop at the cursor column (HTS).
    ///
    /// Routed through the same edit vocabulary `CTC 0` uses, because the
    /// two control functions request the identical edit. TABULATION STOP
    /// MODE scoping, when it lands, has to reach HTS as well.
    pub fn hts(&mut self) {
        self.apply_tab_edit(CharacterTabEdit::SetColumn);
    }

    /// Applies a `TBC` (`CSI Ps g`) parameter; a value only line
    /// tabulation stops answer does nothing.
    pub fn tbc(&mut self, ps: u16) {
        if let Some(edit) = CharacterTabEdit::from_tbc(ps) {
            self.apply_tab_edit(edit);
        }
    }

    /// Applies a `CTC` (`CSI Ps W`) parameter; a value only line
    /// tabulation stops answer does nothing.
    pub fn ctc(&mut self, ps: u16) {
        if let Some(edit) = CharacterTabEdit::from_ctc(ps) {
            self.apply_tab_edit(edit);
        }
    }

    /// Reinstalls the default tabulation stride (DECST8C).
    pub fn decst8c(&mut self) {
        self.tabs.reset();
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
    pub fn viewport_row(&self, line: ViewportLine) -> &Row<Cell> {
        let offset =
            i32::try_from(self.viewport.offset.0).expect("scrollback never exceeds i32::MAX rows");
        self.grid.row(GridLine(i32::from(line.0) - offset))
    }

    /// The write cursor as an emitted frame carries it.
    // TODO: Report the real shape, blink, and visibility once DECSCUSR
    // and DECTCEM land. Block / steady / visible is what the terminal
    // starts at.
    pub fn cursor(&self) -> Cursor {
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

    /// Seats the viewport at `offset`, clamped to the history that
    /// currently exists.
    ///
    /// [`Self::hold_scrolled_viewport`] also writes the offset, so this is
    /// not the only seam that does; it is the seam a future
    /// `DeviceState::scroll` will drive.
    pub fn set_display_offset(&mut self, offset: DisplayOffset) {
        let history =
            u32::try_from(self.grid.history_len()).expect("scrollback never exceeds u32::MAX rows");
        self.viewport.offset = DisplayOffset(offset.0.min(history));
    }

    /// The id of the row the cursor sits on — the anchor a mount samples.
    pub fn cursor_line_id(&self) -> LineId {
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
    pub fn cursor_column(&self) -> GridColumn {
        self.state.column
    }

    /// Applies one tabulation stop edit at the cursor column.
    fn apply_tab_edit(&mut self, edit: CharacterTabEdit) {
        let column = self.state.column;
        match edit {
            CharacterTabEdit::SetColumn => self.tabs.set(column),
            CharacterTabEdit::ClearColumn => self.tabs.clear(column),
            CharacterTabEdit::ClearAllColumns => self.tabs.clear_all(),
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

    /// Seats the cursor at a tabulation column.
    ///
    /// Reports no damage when the column does not change: the call
    /// writes no cell and moves nothing, so the frame it would force
    /// repeats the last one. A move reports [`Damage::Metadata`],
    /// because no cell changed either way.
    ///
    /// # Invariants
    ///
    /// The deferred wrap is deliberately left as it is, unlike
    /// [`Screen::cr`]. Disarming it would make a tab after a full row
    /// seat the cursor back onto the row the application had already
    /// filled.
    fn tab_to(&mut self, column: GridColumn) -> Option<Damage> {
        if self.state.column == column {
            return None;
        }
        self.state.column = column;
        Some(Damage::Metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Color;

    fn screen() -> Screen {
        Screen::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    mod new {
        use super::*;

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
    }

    mod print {
        use super::*;

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
        /// at the start of the next row and damages that row alone.
        ///
        /// The agreed policy leaves the row the wrap left out of the
        /// damage: its contents do not change, and the cursor that moved
        /// off it reaches the renderer through the frame's cursor.
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
            assert_eq!(damage, Some(Damage::rows(ViewportLine(1), ViewportLine(1))));
        }

        /// Asserts that damage is reported in viewport rows, not the grid
        /// rows the write used.
        ///
        /// Case: the user scrolls back one row and the shell echoes a
        /// character at the live tail, which now sits one row lower in the
        /// window.
        #[test]
        fn a_scrolled_screen_reports_damage_in_viewport_rows() {
            let mut screen = screen();
            screen.state.line = ScreenLine(2);
            screen.lf();
            screen.set_display_offset(DisplayOffset(1));
            screen.state.line = ScreenLine(0);
            assert_eq!(
                screen.print('x'),
                Some(Damage::rows(ViewportLine(1), ViewportLine(1)))
            );
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
    }

    mod bs {
        use super::*;

        /// Asserts that a backspace steps the cursor one column left and
        /// reports cursor-only damage.
        ///
        /// Case: a shell line editor erases the character the user just
        /// typed, moving left before overwriting it with a space.
        #[test]
        fn backspace_moves_the_cursor_one_column_left() {
            let mut screen = screen();
            screen.state.column = GridColumn(2);
            let damage = screen.bs();
            assert_eq!(screen.state.column, GridColumn(1));
            assert_eq!(damage, Some(Damage::Metadata));
        }

        /// Asserts that a backspace at column zero leaves the cursor
        /// where it is and reports no damage.
        ///
        /// The agreed policy stops at the left edge rather than wrapping
        /// back onto the previous row. xterm reaches that row only under
        /// reverse-wraparound, which is off by default and additionally
        /// requires autowrap.
        ///
        /// Case: a program emits more backspaces than it printed
        /// characters, running past the start of the line.
        #[test]
        fn a_backspace_at_column_zero_does_not_move() {
            let mut screen = screen();
            assert_eq!(screen.bs(), None);
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that a backspace after a full row both steps back and
        /// disarms the deferred wrap.
        ///
        /// The agreed policy lands one column short of the cell just
        /// written rather than on it. xterm's `CursorBack` decrements
        /// unconditionally without reverse-wraparound and then calls
        /// `ResetWrap`, so the step and the disarm both happen.
        ///
        /// Case: an application fills a row to its last cell and then
        /// backs up to overwrite the character before the last one.
        #[test]
        fn a_backspace_after_a_full_row_steps_back_and_disarms_the_wrap() {
            let mut screen = screen();
            for c in ['a', 'b', 'c', 'd'] {
                screen.print(c);
            }
            assert!(screen.state.pending_wrap);
            screen.bs();
            assert_eq!(screen.state.column, GridColumn(2));
            assert!(!screen.state.pending_wrap);
        }

        /// Asserts that a backspace at column zero still reports damage
        /// while the deferred wrap is armed.
        ///
        /// Case: a one-column screen prints a character, which arms the
        /// wrap without ever leaving column zero, and the application
        /// then emits a backspace.
        #[test]
        fn a_backspace_at_column_zero_disarms_a_pending_wrap() {
            let mut screen = Screen::new(GridSize { cols: 1, rows: 3 }, 10);
            screen.print('x');
            assert!(screen.state.pending_wrap);
            let damage = screen.bs();
            assert_eq!(screen.state.column, GridColumn(0));
            assert!(!screen.state.pending_wrap);
            assert_eq!(damage, Some(Damage::Metadata));
        }
    }

    mod cr {
        use super::*;

        /// Asserts that a carriage return rewinds the column, clears the
        /// deferred-wrap flag, and reports cursor-only damage.
        ///
        /// The agreed policy is [`Damage::Metadata`] rather than the
        /// cursor row: the renderer draws the caret from the frame's
        /// cursor rather than from cell data, so naming the row would
        /// rebuild and re-upload contents that did not change.
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
            assert_eq!(damage, Some(Damage::Metadata));
        }

        /// Asserts that a carriage return with nothing left to rewind
        /// reports no damage at all.
        ///
        /// The agreed policy returns `None` rather than the cursor row or
        /// [`Damage::Metadata`]: both would force a frame that repeats the
        /// one before it, because this call writes no cell and moves the
        /// cursor nowhere.
        ///
        /// Case: a program prints consecutive blank lines, so the `\r` of
        /// each CRLF pair lands on a column the previous pair already
        /// rewound.
        #[test]
        fn a_carriage_return_with_nothing_to_rewind_reports_no_damage() {
            let mut screen = screen();
            assert_eq!(screen.cr(), None);
        }

        /// Asserts that a carriage return at column zero still reports
        /// damage while the deferred wrap is armed.
        ///
        /// Case: a one-column screen prints a character, which arms the
        /// wrap without ever leaving column zero, and the application then
        /// emits a bare `\r`.
        #[test]
        fn a_carriage_return_at_column_zero_disarms_a_pending_wrap() {
            let mut screen = Screen::new(GridSize { cols: 1, rows: 3 }, 10);
            screen.print('x');
            assert_eq!(screen.state.column, GridColumn(0));
            assert!(screen.state.pending_wrap);
            let damage = screen.cr();
            assert!(!screen.state.pending_wrap);
            assert_eq!(damage, Some(Damage::Metadata));
        }
    }

    mod tab_to {
        use super::*;

        /// Asserts that seating the cursor at a new column reports
        /// cursor-only damage.
        ///
        /// The agreed policy is [`Damage::Metadata`] rather than the
        /// cursor row: the renderer draws the caret from the frame's
        /// cursor rather than from cell data, so naming the row would
        /// rebuild and re-upload contents that did not change.
        ///
        /// Case: the shell emits a tab while listing a directory in
        /// aligned columns.
        #[test]
        fn a_tab_that_moves_the_cursor_reports_metadata_damage() {
            let mut screen = screen();
            let damage = screen.tab_to(GridColumn(2));
            assert_eq!(screen.state.column, GridColumn(2));
            assert_eq!(damage, Some(Damage::Metadata));
        }

        /// Asserts that seating the cursor at the column it already
        /// occupies reports no damage at all.
        ///
        /// The agreed policy returns `None` rather than
        /// [`Damage::Metadata`], which would force a frame that repeats
        /// the one before it, because this call writes no cell and
        /// moves the cursor nowhere.
        ///
        /// Case: a tab arrives with the cursor already parked on the
        /// last column, so the clamp hands back the column it started
        /// from.
        #[test]
        fn a_tab_to_the_current_column_reports_no_damage() {
            let mut screen = screen();
            screen.state.column = GridColumn(3);
            assert_eq!(screen.tab_to(GridColumn(3)), None);
        }

        /// Asserts that seating the cursor leaves an armed deferred
        /// wrap alone.
        ///
        /// The agreed policy preserves the flag, unlike [`Screen::cr`].
        /// Disarming it would seat the cursor back onto the row the
        /// application had already filled, which is the behaviour both
        /// VTE and Windows Terminal found real DEC hardware never had.
        ///
        /// Case: an application fills a row to its last cell and then
        /// emits a tab instead of more text.
        #[test]
        fn a_tab_keeps_the_deferred_wrap_armed() {
            let mut screen = screen();
            for c in ['a', 'b', 'c', 'd'] {
                screen.print(c);
            }
            assert!(screen.state.pending_wrap);
            screen.tab_to(GridColumn(0));
            assert!(screen.state.pending_wrap);
        }
    }

    /// Twenty columns put the right edge at 19, so the default stride's
    /// stops at 8 and 16 are reachable and the one at 24 is not.
    fn wide_screen() -> Screen {
        Screen::new(GridSize { cols: 20, rows: 3 }, 10)
    }

    mod ht {
        use super::*;

        /// Asserts that a tab seats the cursor on the next stop.
        ///
        /// Case: the shell emits a tab at the start of a line while
        /// printing aligned columns.
        #[test]
        fn ht_moves_to_the_next_stop() {
            let mut screen = wide_screen();
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(8));
        }

        /// Asserts that a tab past the last reachable stop lands on this
        /// screen's own right edge.
        ///
        /// ECMA-48 § 6.1.7 leaves a movement to a non-existing position
        /// undefined and lists seven options; the agreed policy clamps
        /// to the right edge rather than wrapping to the next line or
        /// refusing the move.
        ///
        /// Case: a twenty-column window shows text that has already run
        /// past the last tab position it can display, and the shell
        /// emits one more tab.
        #[test]
        fn ht_at_the_last_stop_clamps_to_the_screens_own_right_edge() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(16);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(19));
        }

        /// Asserts that a screen too narrow to reach any stop clamps to
        /// its last column.
        ///
        /// Case: the user shrinks the window to four columns and the
        /// shell keeps emitting tabs.
        #[test]
        fn ht_on_a_narrow_screen_clamps_without_reaching_any_stop() {
            let mut screen = screen();
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(3));
        }

        /// Asserts that a tab with nowhere left to go reports no damage.
        ///
        /// The agreed policy returns `None` rather than
        /// [`Damage::Metadata`], which would force a frame that repeats
        /// the one before it, because this call writes no cell and moves
        /// the cursor nowhere.
        ///
        /// Case: a program emits consecutive tabs with the cursor
        /// already parked on the last column.
        #[test]
        fn an_ht_that_does_not_move_reports_no_damage() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(19);
            assert_eq!(screen.ht(), None);
        }
    }

    mod cht {
        use super::*;

        /// Asserts that a counted forward tab skips the stops in
        /// between.
        ///
        /// Case: an application emits `CSI 2 I` to jump two tab
        /// positions in one step.
        #[test]
        fn cht_counts_multiple_stops() {
            let mut screen = wide_screen();
            screen.cht(2);
            assert_eq!(screen.state.column, GridColumn(16));
        }
    }

    mod cbt {
        use super::*;

        /// Asserts that a backward tab seats the cursor on the previous
        /// stop.
        ///
        /// Case: the user presses Shift-Tab to step back to the
        /// previous column of a form.
        #[test]
        fn cbt_moves_back_to_the_previous_stop() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(17);
            screen.cbt(1);
            assert_eq!(screen.state.column, GridColumn(16));
        }

        /// Asserts that a backward tab before the first stop lands on
        /// column zero.
        ///
        /// The agreed policy makes the left edge a fallback rather than
        /// a stop, because the reset stride leaves column zero empty.
        ///
        /// Case: the user presses Shift-Tab near the start of a line,
        /// before the first tab position.
        #[test]
        fn cbt_before_the_first_stop_clamps_to_column_zero() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(5);
            screen.cbt(1);
            assert_eq!(screen.state.column, GridColumn(0));
        }
    }

    mod tab_stop_edits {
        use super::*;

        /// Asserts that a stop set at the cursor is where the next tab
        /// lands.
        ///
        /// Case: an application walks to the column it wants, sets a tab
        /// position there, and returns to the start of the line.
        #[test]
        fn hts_adds_a_stop_the_next_ht_finds() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(3);
            screen.hts();
            screen.state.column = GridColumn(0);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(3));
        }

        /// Asserts that setting a stop leaves the cursor where it was.
        ///
        /// HTS edits the stop table and nothing else; the neighbouring
        /// name HT is the one that moves. Nothing on screen changes
        /// either, which is why `hts` reports no damage to stage.
        ///
        /// Case: an application installs a tab position at the column it
        /// is already writing at, then keeps printing on the same line.
        #[test]
        fn hts_does_not_move_the_cursor() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(3);
            screen.hts();
            assert_eq!(screen.state.column, GridColumn(3));
        }

        /// Asserts that HTS and `CTC 0` install the same stop.
        ///
        /// The two are one edit in the vocabulary rather than two
        /// parallel implementations, so that a later TABULATION STOP
        /// MODE cannot scope one of them and miss the other.
        ///
        /// Case: an application uses CTC rather than HTS to install its
        /// tab positions, having found the CSI form easier to generate.
        #[test]
        fn hts_and_ctc_zero_install_the_same_stop() {
            let mut by_hts = wide_screen();
            by_hts.state.column = GridColumn(3);
            by_hts.hts();

            let mut by_ctc = wide_screen();
            by_ctc.state.column = GridColumn(3);
            by_ctc.ctc(0);

            for screen in [&mut by_hts, &mut by_ctc] {
                screen.state.column = GridColumn(0);
                screen.ht();
            }
            assert_eq!(by_hts.state.column, GridColumn(3));
            assert_eq!(by_ctc.state.column, by_hts.state.column);
        }

        /// Asserts that clearing the stop under the cursor makes the
        /// next tab reach the one after it.
        ///
        /// Case: an application parks on a default tab position and
        /// drops it so its own layout is one column wider.
        #[test]
        fn tbc_zero_clears_the_stop_under_the_cursor() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(8);
            screen.tbc(0);
            screen.state.column = GridColumn(0);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(16));
        }

        /// Asserts that clearing every stop leaves a tab nothing to
        /// find.
        ///
        /// Case: a full-screen application clears the tab table before
        /// installing a layout of its own.
        #[test]
        fn tbc_three_clears_every_stop() {
            let mut screen = wide_screen();
            screen.tbc(3);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(19));
        }

        /// Asserts that a TBC parameter only line tabulation stops
        /// answer leaves the character stops alone.
        ///
        /// The agreed policy drops such a parameter rather than routing
        /// it to the character stops, so a later line-tabulation layer
        /// can claim it without changing what it already did.
        ///
        /// Case: an application written for a printer sends TBC 1 to
        /// drop the line tab stop on the cursor's line.
        #[test]
        fn a_tbc_parameter_only_line_stops_answer_leaves_the_stops_alone() {
            let mut screen = wide_screen();
            screen.tbc(1);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(8));
        }

        /// Asserts that CTC sets and clears the stop under the cursor.
        ///
        /// Case: an application uses CTC rather than HTS and TBC to edit
        /// the tab position it is parked on.
        #[test]
        fn ctc_zero_sets_and_ctc_two_clears_at_the_cursor() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(3);
            screen.ctc(0);
            screen.state.column = GridColumn(0);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(3));

            screen.ctc(2);
            screen.state.column = GridColumn(0);
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(8));
        }

        /// Asserts that DECST8C brings the default stride back after a
        /// full clear.
        ///
        /// Case: an application that cleared the tab table asks for the
        /// default tab positions again before exiting.
        #[test]
        fn decst8c_reinstalls_the_stride_after_a_full_clear() {
            let mut screen = wide_screen();
            screen.tbc(3);
            screen.decst8c();
            screen.ht();
            assert_eq!(screen.state.column, GridColumn(8));
        }
    }

    mod lf {
        use super::*;

        /// Asserts that a linefeed above the bottom row only moves the
        /// cursor and reports cursor-only damage.
        ///
        /// The agreed policy is [`Damage::Metadata`] rather than the
        /// departed and arrived rows: neither row's contents change, and
        /// the renderer draws the caret from the frame's cursor.
        ///
        /// Case: a shell prints multiple output lines while the screen
        /// still has empty rows below the cursor.
        #[test]
        fn a_linefeed_above_the_bottom_moves_the_cursor() {
            let mut screen = screen();
            let damage = screen.lf();
            assert_eq!(screen.state.line, ScreenLine(1));
            assert_eq!(damage, Some(Damage::Metadata));
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
    }

    mod ri {
        use super::*;

        /// Asserts that a reverse index below the top margin moves the
        /// cursor up one row without scrolling.
        ///
        /// Case: a full-screen application walks its cursor back up the
        /// screen a line at a time.
        #[test]
        fn a_reverse_index_below_the_top_moves_the_cursor_up() {
            let mut screen = screen();
            screen.state.line = ScreenLine(2);
            let damage = screen.ri();
            assert_eq!(screen.state.line, ScreenLine(1));
            assert_eq!(damage, Some(Damage::Metadata));
        }

        /// Asserts that a reverse index at the top margin scrolls the
        /// screen down instead of moving the cursor.
        ///
        /// ECMA-48 § 6.1.7 leaves a movement past the first line undefined
        /// and lists seven permitted behaviours; the agreed policy takes
        /// (f), scrolling, rather than blocking the position or leaving
        /// the cursor where it is.
        ///
        /// Case: a pager scrolls backwards with its cursor already parked
        /// on the first line of the screen.
        #[test]
        fn a_reverse_index_at_the_top_margin_scrolls_the_screen_down() {
            let mut screen = screen();
            screen.grid[ScreenLine(0)][0].c = 'a';
            let damage = screen.ri();
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.grid[ScreenLine(1)][0].c, 'a');
            assert_eq!(damage, Some(Damage::Full));
        }

        /// Asserts that a reverse index disarms the deferred wrap on both
        /// the moving and the scrolling path.
        ///
        /// The agreed policy follows xterm and VTE, whose reverse index
        /// reaches its cursor-up helper on both paths and resets the flag
        /// there. It is a deliberate divergence from ghostty, kitty, and
        /// wezterm, which clear it only when the cursor moves, and from
        /// alacritty, which clears it on neither — and `Screen::lf`
        /// preserves the flag, so the split is not accidental.
        ///
        /// Case: a program fills the last column of a row and then emits a
        /// reverse index instead of the newline the pending wrap was
        /// waiting for.
        #[test]
        fn a_reverse_index_disarms_the_deferred_wrap_on_both_paths() {
            let mut moved = screen();
            moved.state.line = ScreenLine(1);
            moved.state.pending_wrap = true;
            moved.ri();
            assert!(!moved.state.pending_wrap);

            let mut scrolled = screen();
            scrolled.state.pending_wrap = true;
            scrolled.ri();
            assert!(!scrolled.state.pending_wrap);
        }

        /// Asserts that the row exposed at the top carries the pen's
        /// background.
        ///
        /// Case: an application paints a coloured panel and scrolls it
        /// backwards, expecting the newly exposed row to match rather than
        /// show the terminal default.
        #[test]
        fn the_exposed_row_carries_the_pen_background() {
            let mut screen = screen();
            screen.pen_mut().bg = Color::Indexed(4);
            screen.ri();
            assert_eq!(screen.grid[ScreenLine(0)][0].bg, Color::Indexed(4));
        }

        /// Asserts that a reverse index leaves a scrolled-back viewport
        /// showing what it was showing.
        ///
        /// The agreed policy leaves the display offset alone rather than
        /// adjusting it the way `Screen::lf` does. A forward scroll grows
        /// history, so holding the view still requires moving the offset;
        /// a reverse scroll leaves history untouched, so moving the offset
        /// would push the viewport onto different history instead.
        ///
        /// Case: the user has scrolled back to read earlier output while a
        /// full-screen application keeps scrolling its own view backwards.
        #[test]
        fn a_reverse_index_leaves_a_scrolled_viewport_where_it_is() {
            let mut screen = screen();
            screen.grid[ScreenLine(0)][0].c = 'a';
            screen.state.line = ScreenLine(2);
            screen.lf();
            screen.set_display_offset(DisplayOffset(1));
            let showing = screen.viewport_row(ViewportLine(0))[0].c;
            screen.state.line = ScreenLine(0);
            screen.ri();
            assert_eq!(screen.display_offset(), DisplayOffset(1));
            assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, showing);
        }

        /// Asserts that a reverse index with the cursor above a non-zero
        /// top margin, already on the first row, moves and scrolls nothing.
        ///
        /// The agreed policy follows DEC STD 070 and xterm: a cursor that
        /// hits the screen edge outside the scrolling region stays put,
        /// rather than scrolling the region it is not inside.
        ///
        /// Case: an application sets a scroll region below a status line
        /// and emits a reverse index while the cursor sits on that status
        /// line.
        #[test]
        fn a_reverse_index_above_a_top_margin_at_row_zero_does_nothing() {
            let mut screen = screen();
            screen.margins.top = ScreenLine(1);
            screen.grid[ScreenLine(0)][0].c = 'a';
            let damage = screen.ri();
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(damage, None);
        }

        /// Asserts that a reverse index at a non-zero top margin scrolls
        /// the region and leaves the rows above it alone.
        ///
        /// Case: an application keeps a status line on the first row and
        /// scrolls the pane below it backwards.
        #[test]
        fn a_reverse_index_at_a_top_margin_scrolls_only_the_region() {
            let mut screen = screen();
            screen.margins.top = ScreenLine(1);
            for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c')] {
                screen.grid[ScreenLine(line)][0].c = glyph;
            }
            screen.state.line = ScreenLine(1);
            let blank = screen.state.pen.erase_cell().c;
            let damage = screen.ri();
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, blank);
            assert_eq!(screen.grid[ScreenLine(2)][0].c, 'b');
            assert_eq!(damage, Some(Damage::Full));
        }
    }

    mod erase_in_line {
        use super::*;

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
    }

    mod erase_in_display {
        use super::*;

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
    }

    mod cursor {
        use super::*;

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
    }

    mod viewport_row {
        use super::*;

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
    }

    mod display_offset {
        use super::*;

        /// Asserts that output arriving while the user is scrolled back
        /// leaves the viewed content where it was.
        ///
        /// The agreed policy holds the viewport still rather than letting
        /// it drift with the live tail: DECSET 1010 (`scrollTtyOutput`)
        /// stays off, so the viewport holds its position while the PTY
        /// emits output, and every terminal that keeps scrollback
        /// behaves this way.
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
    }

    mod viewport_row_of {
        use super::*;

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
}
