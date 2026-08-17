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

    #[expect(dead_code, reason = "merge will be used when multiple effects combine")]
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
    pub fn build(size: GridSize, max_history: usize) -> Self {
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
        Effects {
            damage: Some(Damage::Full),
            history: Some(history),
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
        Screen::build(GridSize { cols: 4, rows: 3 }, 10)
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
        let mut screen = Screen::build(GridSize { cols: 4, rows: 3 }, 1);
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
    /// Case: an application sets a colored background and scrolls, and
    /// the freshly exposed bottom row must show that background (BCE),
    /// not the default one.
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
}
