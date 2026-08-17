//! The atomic grid + cursor operation unit for one terminal screen.
//!
//! [`Screen`] owns cell storage ([`grid::Grid`]) and the write cursor,
//! and updates them together; every mutation returns its observable
//! [`Effects`] for the caller to stage instead of staging internally.

pub mod cell;
pub mod grid;

use self::cell::Pen;
use self::grid::{Grid, HistoryEvent};
use crate::schema::{Damage, DisplayOffset, GridSize};

/// One mutation's observable effects, for the caller to stage.
///
/// Operations return their damage and history effect instead of
/// staging them internally; the future executor folds these into the
/// damage ledger and the placement store.
#[derive(Debug, Default, PartialEq)]
pub struct Effects {
    damage: Option<Damage>,
    history: Option<HistoryEvent>,
}

impl Effects {
    fn damage_rows(rows: Vec<u16>) -> Self {
        Self {
            damage: Some(Damage::Delta(rows.into())),
            history: None,
        }
    }

    fn full(history: Option<HistoryEvent>) -> Self {
        Self {
            damage: Some(Damage::Full),
            history,
        }
    }

    fn merge(&mut self, other: Effects) {
        match (&mut self.damage, other.damage) {
            (Some(mine), Some(theirs)) => *mine |= theirs,
            (mine @ None, theirs) => *mine = theirs,
            (_, None) => {}
        }
        debug_assert!(
            self.history.is_none() || other.history.is_none(),
            "one operation produces at most one history event"
        );
        if other.history.is_some() {
            self.history = other.history;
        }
    }
}

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
    write: WriteState,
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
        debug_assert!(
            size.cols > 0 && size.rows > 0,
            "degenerate grid sizes are rejected by the caller"
        );
        Self {
            grid: Grid::build(size, max_history),
            viewport: Viewport {
                offset: DisplayOffset(0),
            },
            write: WriteState::default(),
            saved: SavedCursorSlots::default(),
            margins: Margins {
                top: 0,
                bottom: size.rows - 1,
            },
        }
    }

    /// Prints one character at the cursor with the current pen,
    /// wrapping first when the deferred wrap is armed.
    ///
    /// The caller dispatches control bytes itself; this method assumes
    /// a printable character of display width one.
    pub fn print(&mut self, c: char) -> Effects {
        let mut effects = Effects::default();
        if self.write.pending_wrap {
            self.write.pending_wrap = false;
            self.write.column = 0;
            effects.merge(self.linefeed());
        } else {
            effects.merge(Effects::damage_rows(vec![self.write.line]));
        }
        *self.grid.cell_mut(self.write.line, self.write.column) = self.write.pen.stamp(c);
        if self.write.column + 1 < self.grid.size().cols {
            self.write.column += 1;
        } else {
            self.write.pending_wrap = true;
        }
        effects
    }

    /// Rewinds the cursor to column zero and disarms the deferred
    /// wrap.
    pub fn carriage_return(&mut self) -> Effects {
        self.write.column = 0;
        self.write.pending_wrap = false;
        Effects::damage_rows(vec![self.write.line])
    }

    /// Moves the cursor down one row, scrolling at the bottom margin;
    /// the deferred-wrap flag is deliberately preserved.
    pub fn linefeed(&mut self) -> Effects {
        if self.write.line < self.margins.bottom {
            let departed = self.write.line;
            self.write.line += 1;
            return Effects::damage_rows(vec![departed, self.write.line]);
        }
        let history = self.grid.scroll_up_one(self.write.pen.erase_cell());
        Effects::full(Some(history))
    }

    /// Erases part of the cursor row with the pen background (BCE);
    /// [`EraseLineMode::ToEnd`] is a no-op while the deferred wrap is
    /// armed.
    pub fn erase_in_line(&mut self, mode: EraseLineMode) -> Effects {
        if matches!(mode, EraseLineMode::ToEnd) && self.write.pending_wrap {
            return Effects::default();
        }
        let cols = self.grid.size().cols;
        let columns = match mode {
            EraseLineMode::ToEnd => self.write.column..cols,
            EraseLineMode::ToStart => 0..self.write.column + 1,
            EraseLineMode::All => 0..cols,
        };
        self.grid
            .fill_visible_row_range(self.write.line, columns, self.write.pen.erase_cell());
        Effects::damage_rows(vec![self.write.line])
    }

    /// Erases part of the visible screen with the pen background
    /// (BCE), in place; scrollback history is never touched.
    pub fn erase_in_display(&mut self, mode: EraseScreenMode) -> Effects {
        let GridSize { cols, rows } = self.grid.size();
        let blank = self.write.pen.erase_cell();
        match mode {
            EraseScreenMode::Below => {
                self.grid
                    .fill_visible_row_range(self.write.line, self.write.column..cols, blank);
                for line in self.write.line + 1..rows {
                    self.grid.fill_visible_row_range(line, 0..cols, blank);
                }
                Effects::damage_rows((self.write.line..rows).collect())
            }
            EraseScreenMode::Above => {
                for line in 0..self.write.line {
                    self.grid.fill_visible_row_range(line, 0..cols, blank);
                }
                self.grid
                    .fill_visible_row_range(self.write.line, 0..self.write.column + 1, blank);
                Effects::damage_rows((0..=self.write.line).collect())
            }
            EraseScreenMode::All => {
                for line in 0..rows {
                    self.grid.fill_visible_row_range(line, 0..cols, blank);
                }
                Effects::full(None)
            }
        }
    }

    /// Mutably borrows the SGR pen; applying SGR sequences is the
    /// caller's job.
    pub fn pen_mut(&mut self) -> &mut Pen {
        &mut self.write.pen
    }

    /// Number of scrollback rows the viewport sits above the live
    /// tail; always zero until scroll operations arrive.
    pub fn display_offset(&self) -> DisplayOffset {
        self.viewport.offset
    }
}

/// Cursor state saved by DECSC, restored by DECRC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedCursor {
    /// Saved cursor row within the visible screen.
    pub line: u16,
    /// Saved cursor column.
    pub column: u16,
    /// Saved SGR pen.
    pub pen: Pen,
    /// Saved deferred-wrap flag.
    pub pending_wrap: bool,
}

/// Per-screen save slots for DECSC (the ANSI slot arrives later).
#[derive(Default)]
pub struct SavedCursorSlots {
    /// The DECSC slot; `None` until a save happens.
    pub dec: Option<SavedCursor>,
}

/// DECSTBM scroll region; `bottom` is the inclusive last row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Margins {
    /// First row of the scroll region (0 = top of screen).
    pub top: u16,
    /// Inclusive last row of the scroll region (default `rows - 1`).
    pub bottom: u16,
}

struct Viewport {
    offset: DisplayOffset,
}

#[derive(Default)]
struct WriteState {
    line: u16,
    column: u16,
    pending_wrap: bool,
    pen: Pen,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Color;

    fn screen() -> Screen {
        Screen::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Asserts that a fresh screen starts at the origin, pinned to the
    /// live tail, with an empty history.
    ///
    /// Case: a terminal spawns and the first shell output must land at
    /// the top-left of an unscrolled screen.
    #[test]
    fn a_fresh_screen_starts_at_the_origin() {
        let screen = screen();
        assert_eq!((screen.write.line, screen.write.column), (0, 0));
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
        screen.write.column = 2;
        screen.write.pending_wrap = true;
        let effects = screen.carriage_return();
        assert_eq!(screen.write.column, 0);
        assert!(!screen.write.pending_wrap);
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![0].into())),
                history: None,
            }
        );
    }

    /// Asserts that a linefeed above the bottom row only moves the
    /// cursor, damaging the departed and arrived rows.
    ///
    /// Case: a shell prints multiple output lines while the screen
    /// still has empty rows below the cursor.
    #[test]
    fn a_linefeed_above_the_bottom_moves_the_cursor() {
        let mut screen = screen();
        let effects = screen.linefeed();
        assert_eq!(screen.write.line, 1);
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![0, 1].into())),
                history: None,
            }
        );
    }

    /// Asserts that a bottom-row linefeed scrolls the screen, pushes
    /// the top row into history, and reports full damage.
    ///
    /// Case: a shell at the last row keeps printing, and the oldest
    /// visible line must survive as scrollback history.
    #[test]
    fn a_bottom_linefeed_scrolls_and_pushes_history() {
        let mut screen = screen();
        screen.write.line = 2;
        let effects = screen.linefeed();
        assert_eq!(screen.write.line, 2);
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Full),
                history: Some(HistoryEvent::Pushed),
            }
        );
        assert_eq!(screen.grid.history_len(), 1);
    }

    /// Asserts that at history capacity a bottom-row linefeed reports
    /// the eviction.
    ///
    /// Case: a long-running session has filled the scrollback limit,
    /// and continued output starts dropping the oldest history.
    #[test]
    fn a_bottom_linefeed_at_capacity_reports_the_eviction() {
        let mut screen = Screen::new(GridSize { cols: 4, rows: 3 }, 1);
        screen.write.line = 2;
        screen.linefeed();
        let effects = screen.linefeed();
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Full),
                history: Some(HistoryEvent::PushedWithEviction),
            }
        );
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
        screen.write.line = 2;
        screen.linefeed();
        assert_eq!(screen.grid.cell(2, 0).bg, Color::Indexed(4));
        assert_eq!(screen.grid.cell(2, 3).bg, Color::Indexed(4));
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
        screen.write.pending_wrap = true;
        screen.linefeed();
        assert!(screen.write.pending_wrap);
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
        let effects = screen.print('a');
        assert_eq!(screen.grid.cell(0, 0).c, 'a');
        assert_eq!(screen.grid.cell(0, 0).fg, Color::Indexed(1));
        assert_eq!((screen.write.line, screen.write.column), (0, 1));
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![0].into())),
                history: None,
            }
        );
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
        screen.write.column = 3;
        screen.print('x');
        assert_eq!(screen.grid.cell(0, 3).c, 'x');
        assert_eq!(screen.write.column, 3);
        assert!(screen.write.pending_wrap);
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
        let effects = screen.print('e');
        assert_eq!(screen.grid.cell(1, 0).c, 'e');
        assert_eq!((screen.write.line, screen.write.column), (1, 1));
        assert!(!screen.write.pending_wrap);
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![0, 1].into())),
                history: None,
            }
        );
    }

    /// Asserts that a deferred wrap on the bottom row scrolls the
    /// screen and reports the history push with full damage.
    ///
    /// Case: a shell fills the very last cell of the screen and keeps
    /// printing, forcing a scroll in the middle of the wrap.
    #[test]
    fn a_wrap_on_the_bottom_row_scrolls() {
        let mut screen = screen();
        screen.write.line = 2;
        screen.write.column = 3;
        screen.print('x');
        let effects = screen.print('y');
        assert_eq!(screen.grid.cell(2, 0).c, 'y');
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Full),
                history: Some(HistoryEvent::Pushed),
            }
        );
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
        screen.write.column = 1;
        screen.pen_mut().bg = Color::Indexed(2);
        let effects = screen.erase_in_line(EraseLineMode::ToEnd);
        assert_eq!(screen.grid.cell(0, 0).c, 'a');
        assert_eq!(screen.grid.cell(0, 1).c, ' ');
        assert_eq!(screen.grid.cell(0, 1).bg, Color::Indexed(2));
        assert_eq!(screen.grid.cell(0, 3).bg, Color::Indexed(2));
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![0].into())),
                history: None,
            }
        );
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
        screen.write.column = 1;
        screen.erase_in_line(EraseLineMode::ToStart);
        assert_eq!(screen.grid.cell(0, 0).c, ' ');
        assert_eq!(screen.grid.cell(0, 1).c, ' ');
        assert_eq!(screen.grid.cell(0, 2).c, 'c');
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
        let effects = screen.erase_in_line(EraseLineMode::ToEnd);
        assert_eq!(screen.grid.cell(0, 3).c, 'd');
        assert_eq!(effects, Effects::default());
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
        screen.write.column = 1;
        screen.erase_in_line(EraseLineMode::All);
        assert_eq!(screen.grid.cell(0, 0).c, ' ');
        assert_eq!(screen.grid.cell(0, 2).c, ' ');
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
        screen.linefeed();
        screen.carriage_return();
        for c in ['b', 'c'] {
            screen.print(c);
        }
        screen.write.column = 1;
        let effects = screen.erase_in_display(EraseScreenMode::Below);
        assert_eq!(screen.grid.cell(0, 0).c, 'a');
        assert_eq!(screen.grid.cell(1, 0).c, 'b');
        assert_eq!(screen.grid.cell(1, 1).c, ' ');
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![1, 2].into())),
                history: None,
            }
        );
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
        screen.linefeed();
        screen.carriage_return();
        for c in ['b', 'c', 'd'] {
            screen.print(c);
        }
        screen.write.column = 1;
        let effects = screen.erase_in_display(EraseScreenMode::Above);
        assert_eq!(screen.grid.cell(0, 0).c, ' ');
        assert_eq!(screen.grid.cell(1, 0).c, ' ');
        assert_eq!(screen.grid.cell(1, 1).c, ' ');
        assert_eq!(screen.grid.cell(1, 2).c, 'd');
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Delta(vec![0, 1].into())),
                history: None,
            }
        );
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
        screen.write.line = 2;
        screen.linefeed();
        screen.carriage_return();
        for c in ['a', 'b'] {
            screen.print(c);
        }
        let effects = screen.erase_in_display(EraseScreenMode::All);
        assert_eq!(screen.grid.cell(2, 0).c, ' ');
        assert_eq!(screen.grid.cell(2, 1).c, ' ');
        assert_eq!(screen.grid.history_len(), 1);
        assert_eq!(
            effects,
            Effects {
                damage: Some(Damage::Full),
                history: None,
            }
        );
    }
}
