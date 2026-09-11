//! The atomic grid + cursor operation unit for one terminal screen.
//!
//! [`Screen`] owns cell storage ([`grid::Grid`]) and the write cursor,
//! and updates them together; a mutation that damages rows returns the
//! [`DamageSpan`] it produced for the caller to stage, and pure cursor
//! motion returns nothing, because the per-chunk cursor diff reports
//! it.

pub mod cell;
pub mod character_sets;
pub mod checkpoint;
pub mod grid;
pub mod margins;
pub mod selection;
pub mod tabs;
pub mod viewport;

pub(crate) mod cursor;
pub(crate) mod placements;

mod state;

use self::cell::{Cell, Pen};
use self::grid::Grid;
use self::grid::LineId;
use self::grid::row::Row;
use crate::device::modes::{AutoWrap, InsertReplaceMode, TextCursorEnable};
use crate::frame::damage::DamageSpan;
use crate::placement::{AnchoredPlacement, InstanceId, PlacementSize};
use crate::screen::character_sets::{
    CharacterSet, CharacterSetMapping, GCode, GraphicChar, SingleShift,
};
use crate::screen::checkpoint::Checkpoint;
use crate::screen::cursor::{Cursor, CursorShape};
use crate::screen::grid::GridSize;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
use crate::screen::margins::{Margins, OriginMode, ScrollRegion};
use crate::screen::placements::ScreenPlacements;
use crate::screen::selection::{
    CellSide, Resolved, ScreenSelection, SelectionEnd, SelectionKind, SelectionRange,
};
use crate::screen::state::ScreenState;
use crate::screen::tabs::{CharacterTabEdit, TabStops};
use crate::screen::viewport::{DisplayOffset, Scroll, Viewport, ViewportLine};
use std::ops::Range;

/// One terminal screen: cell storage plus the write cursor, updated
/// atomically by each operation.
///
/// # Invariants
///
/// Both grid axes are nonzero; degenerate sizes are rejected by the
/// caller (the same contract as [`crate::Vt::resize`]).
#[derive(Debug)]
pub struct Screen {
    grid: Grid,
    viewport: Viewport,
    state: ScreenState,
    scroll_region: ScrollRegion,
    tabs: TabStops,
    character_set_mapping: CharacterSetMapping,
    checkpoint: Checkpoint,
    placements: ScreenPlacements,
    selection: ScreenSelection,
}

/// Span selector for [`Screen::erase_in_line`] (`CSI K`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EraseLineMode {
    /// From the cursor to the end of the row (`EL 0`).
    ToEnd,
    /// From the start of the row through the cursor column (`EL 1`).
    ToStart,
    /// The whole row (`EL 2`).
    All,
}

/// Span selector for [`Screen::erase_in_display`] (`CSI J`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EraseScreenMode {
    /// From the cursor cell to the end of the screen (`ED 0`).
    Below,
    /// From the top of the screen through the cursor cell (`ED 1`).
    Above,
    /// The whole visible screen (`ED 2`); history is untouched.
    All,
}

impl EraseLineMode {
    /// The span an `EL` (`CSI Ps K`) parameter selects; `None` for a
    /// value this terminal does not answer.
    pub fn from_el(ps: u16) -> Option<Self> {
        match ps {
            0 => Some(Self::ToEnd),
            1 => Some(Self::ToStart),
            2 => Some(Self::All),
            _ => None,
        }
    }
}

impl EraseScreenMode {
    /// The span an `ED` (`CSI Ps J`) parameter selects; `None` for a
    /// value this terminal does not answer.
    ///
    /// `ED 3` erases the scrollback, which this terminal does not model:
    /// every span here is confined to the visible screen.
    pub fn from_ed(ps: u16) -> Option<Self> {
        match ps {
            0 => Some(Self::Below),
            1 => Some(Self::Above),
            2 => Some(Self::All),
            _ => None,
        }
    }
}

/// Construction.
impl Screen {
    /// Builds a blank screen with the cursor at the origin and the
    /// viewport pinned to the live tail.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            scroll_region: ScrollRegion::new(size.rows),
            grid: Grid::new(size, max_history),
            viewport: Viewport::default(),
            state: ScreenState::default(),
            tabs: TabStops::default(),
            character_set_mapping: CharacterSetMapping::default(),
            checkpoint: Checkpoint::default(),
            placements: ScreenPlacements::new(),
            selection: ScreenSelection::new(),
        }
    }
}

/// Graphic character output.
impl Screen {
    /// Prints one character at the cursor with the current pen, wrapping
    /// first when the deferred wrap is armed and autowrap is set.
    ///
    /// `c` must be a printable character of display width one.
    ///
    /// `insert_replace` is `IRM`: under [`InsertReplaceMode::Insert`] the
    /// rest of the row shifts right one column before the character lands.
    ///
    /// `auto_wrap` is `DECAWM`. While it is reset, a character at the right
    /// border replaces the last column, and an armed wrap is not resolved
    /// either, because a `DECRC` can restore one.
    ///
    /// Reports [`DamageSpan::Full`] when the wrap scrolled, and otherwise
    /// the row the character landed on, or `None` when that row has
    /// scrolled out of the window.
    pub fn print(
        &mut self,
        c: char,
        insert_replace: InsertReplaceMode,
        auto_wrap: AutoWrap,
    ) -> Option<DamageSpan> {
        let GraphicChar(glyph) = self.character_set_mapping.translate(c);
        let wrapping = auto_wrap.wraps();
        let wrap = if self.state.pending_wrap && wrapping {
            self.state.column = GridColumn(0);
            self.line_feed()
        } else {
            None
        };
        // NOTE: The shift runs after the deferred wrap is resolved and
        // before the glyph lands. Moving it above the wrap would let
        // `insert_characters` clear `pending_wrap`, and the character
        // would overwrite the last column instead of wrapping to the
        // next row.
        if matches!(insert_replace, InsertReplaceMode::Insert) {
            self.insert_characters(1);
        }
        self.grid[self.state.line][self.state.column] = self.state.pen.stamp(glyph);
        let at_right_edge = self.at_right_edge();
        if !at_right_edge {
            self.state.column.0 += 1;
        }
        self.state.pending_wrap = at_right_edge && wrapping;
        match wrap {
            Some(DamageSpan::Full) => Some(DamageSpan::Full),
            _ => self.damage_span(self.state.line, self.state.line),
        }
    }

    /// Disarms the deferred wrap, leaving the cursor and the cells
    /// alone.
    ///
    /// This is the half of `DECRST 7` that `Screen` owns. The saved
    /// cursor keeps its own flag: DEC STD-070 has `DECSC` carry the
    /// last-column flag, so a reset of the mode must not reach it.
    pub fn disarm_pending_wrap(&mut self) {
        self.state.pending_wrap = false;
    }
}

/// Cursor addressing.
///
/// None of these report damage. A move that only repositions the write
/// cursor is carried by the per-chunk cursor diff, so returning a
/// `DamageSpan` would repaint rows that did not change.
impl Screen {
    /// Moves the cursor one column left and disarms the deferred wrap.
    ///
    /// A backspace at column zero stays there: xterm reaches the
    /// previous row only under reverse-wraparound, which is off by
    /// default. Cursor motion reaches the renderer through the
    /// per-chunk cursor diff, so nothing is reported here.
    ///
    /// # Control Functions
    ///
    /// - `BS` (`0x08`)
    pub fn backspace(&mut self) {
        self.move_cursor_left(1);
    }

    /// Moves the cursor up `count` rows in the same column, never
    /// scrolling.
    ///
    /// The top margin is the barrier: a cursor at or below it stops
    /// there, and only a cursor already above it reaches the first row.
    ///
    /// `DECOM` needs no branch here. Setting it seats the cursor inside
    /// the vertical region, and this clamp keeps it there, so a cursor
    /// origin mode confined can never step out of the region.
    ///
    /// # Control Functions
    ///
    /// - `CUU` (`CSI Pn A`)
    /// - `CPL` (`CSI Pn F`) — before its carriage return
    pub fn move_cursor_up(&mut self, count: u16) {
        let top = self.scroll_region.top_margin();
        let limit = if self.state.line >= top {
            top
        } else {
            ScreenLine(0)
        };
        self.state.line = ScreenLine(self.state.line.0.saturating_sub(count).max(limit.0));
        self.state.pending_wrap = false;
    }

    /// Moves the cursor down `count` rows in the same column, never
    /// scrolling.
    ///
    /// The bottom margin is the barrier, mirroring
    /// [`Self::move_cursor_up`]: a cursor at or above it stops there,
    /// and only a cursor already below it reaches the last row.
    ///
    /// # Control Functions
    ///
    /// - `CUD` (`CSI Pn B`)
    /// - `CNL` (`CSI Pn E`) — before its carriage return
    pub fn move_cursor_down(&mut self, count: u16) {
        let bottom = self.scroll_region.bottom_margin();
        let limit = if self.state.line <= bottom {
            bottom
        } else {
            ScreenLine(self.grid.size().rows - 1)
        };
        self.state.line = ScreenLine(self.state.line.0.saturating_add(count).min(limit.0));
        self.state.pending_wrap = false;
    }

    /// Moves the cursor `count` columns left, stopping at the first
    /// column.
    ///
    /// The page border is the barrier, not a margin: this terminal has
    /// no left margin, because `DECSLRM` needs the vertical split screen
    /// mode it does not implement.
    ///
    /// # Control Functions
    ///
    /// - `CUB` (`CSI Pn D`)
    /// - `BS` (`0x08`) — with a count of one
    pub fn move_cursor_left(&mut self, count: u16) {
        self.seat_column(GridColumn(self.state.column.0.saturating_sub(count)));
    }

    /// Moves the cursor `count` columns right, stopping at the last
    /// column.
    ///
    /// The page border is the barrier, mirroring
    /// [`Self::move_cursor_left`].
    ///
    /// # Control Functions
    ///
    /// - `CUF` (`CSI Pn C`)
    /// - `HPR` (`CSI Pn a`)
    pub fn move_cursor_right(&mut self, count: u16) {
        self.seat_column(GridColumn(self.state.column.0.saturating_add(count)));
    }

    /// Rewinds the cursor to column zero and disarms the deferred wrap.
    ///
    /// # Control Functions
    ///
    /// - `CR` (`0x0D`)
    /// - `NEL` (`0x85`, `ESC E`) — its first half
    pub fn carriage_return(&mut self) {
        self.seat_column(GridColumn(0));
    }

    /// Addresses the cursor at a one-based line and column, `None` for
    /// an omitted parameter.
    ///
    /// A zero addresses the first line or column, the same as a one.
    /// [`Self::seat_cursor`] resolves the line against the origin mode
    /// and clamps both axes, so a line outside the addressable region
    /// stops at its edge rather than being refused.
    ///
    /// # Control Functions
    ///
    /// - `CUP` (`CSI Pl ; Pc H`)
    /// - `HVP` (`CSI Pl ; Pc f`)
    pub fn move_cursor_to(&mut self, line: Option<u16>, column: Option<u16>) {
        self.seat_cursor(
            ScreenLine(Self::addressed_index(line)),
            GridColumn(Self::addressed_index(column)),
        );
    }

    /// Addresses the cursor at a one-based column on the current line,
    /// `None` for an omitted parameter.
    ///
    /// A zero addresses the first column, the same as a one, and a column
    /// past the last stops there. The row is never touched: this seats the
    /// column alone, so neither origin resolution nor a line clamp can move
    /// the cursor off the row it is on. [`Self::seat_column`] also discards
    /// a pending deferred wrap, the same disarm the other addressing
    /// methods perform.
    ///
    /// # Control Functions
    ///
    /// - `CHA` (`CSI Pn G`)
    /// - `HPA` (``CSI Pn ` ``)
    pub fn move_cursor_to_column(&mut self, column: Option<u16>) {
        self.seat_column(GridColumn(Self::addressed_index(column)));
    }

    /// Addresses the cursor at a one-based line in the current column,
    /// `None` for an omitted parameter.
    ///
    /// A zero addresses the first line, the same as a one. The line is
    /// resolved against the current [`OriginMode`] and clamped, so a line
    /// past the addressable region stops at its edge rather than being
    /// refused. The column is never touched, but [`Self::seat_line`] still
    /// discards a pending deferred wrap, the same disarm the other
    /// addressing methods perform.
    ///
    /// # Control Functions
    ///
    /// - `VPA` (`CSI Pn d`)
    pub fn move_cursor_to_line(&mut self, line: Option<u16>) {
        self.seat_line(ScreenLine(Self::addressed_index(line)));
    }

    /// Seats the cursor at `line` — measured from the origin the current
    /// [`OriginMode`] defines — and `column`, clamping both axes and
    /// disarming the deferred wrap. The disarm follows xterm, whose
    /// `CursorSet` ends in `ResetWrap`, unlike a linefeed, which
    /// preserves the wrap on purpose.
    ///
    /// This composes the two single-axis helpers, [`Self::seat_line`] and
    /// [`Self::seat_column`], which absolute single-axis addressing
    /// reaches directly, so the origin and each clamp are decided in one
    /// place apiece and cannot drift. Vertical relative motion, line
    /// feeding, and tabulation keep their own barriers and stay outside
    /// these helpers on purpose.
    fn seat_cursor(&mut self, line: ScreenLine, column: GridColumn) {
        self.seat_line(line);
        self.seat_column(column);
    }

    /// Seats the cursor at `line`, measured from the origin the current
    /// [`OriginMode`] defines, clamping it to the addressable region and
    /// disarming the deferred wrap, without touching the column.
    fn seat_line(&mut self, line: ScreenLine) {
        let GridSize { rows, .. } = self.grid.size();
        let (origin, last) = match self.scroll_region.origin_mode() {
            OriginMode::WithinMargins => (
                self.scroll_region.top_margin(),
                self.scroll_region.bottom_margin(),
            ),
            OriginMode::UpperLeftCorner => (ScreenLine(0), ScreenLine(rows - 1)),
        };
        self.state.line = ScreenLine(line.0.saturating_add(origin.0).min(last.0));
        self.state.pending_wrap = false;
    }

    /// Seats the cursor at `column`, clamping it to the page and
    /// disarming the deferred wrap, without touching the line.
    fn seat_column(&mut self, column: GridColumn) {
        let cols = self.grid.size().cols;
        self.state.column = GridColumn(column.0.min(cols - 1));
        self.state.pending_wrap = false;
    }

    /// The zero-based index a one-based addressing parameter names, where
    /// an omitted parameter and an explicit zero both name the first
    /// position.
    fn addressed_index(parameter: Option<u16>) -> u16 {
        match parameter {
            None | Some(0) => 0,
            Some(position) => position - 1,
        }
    }
}

/// Line feeding, region scrolling, and in-row character editing.
impl Screen {
    /// Moves the cursor down one row, scrolling at the bottom margin;
    /// the deferred-wrap flag is deliberately preserved.
    ///
    /// A move inside the screen reports nothing: neither the departed nor
    /// the arrived row changes contents, and the cursor motion reaches the
    /// renderer through the per-chunk cursor diff. Scrolling moves content
    /// and reports [`DamageSpan::Full`].
    ///
    /// A cursor below a non-zero bottom margin and already on the last
    /// row moves nothing and scrolls nothing.
    ///
    /// [`Self::print`] also calls this to complete a deferred wrap, so
    /// the operation is not reached only from a control function.
    ///
    /// # Control Functions
    ///
    /// - `LF` (`0x0A`)
    /// - `VT` (`0x0B`)
    /// - `FF` (`0x0C`)
    /// - `IND` (`0x84`, `ESC D`)
    /// - `NEL` (`0x85`, `ESC E`) — after the carriage return
    pub fn line_feed(&mut self) -> Option<DamageSpan> {
        if self.state.line == self.scroll_region.bottom_margin() {
            return self.scroll_region_up(1);
        }
        if self.state.line.0 + 1 < self.grid.size().rows {
            self.state.line.0 += 1;
        }
        None
    }

    /// Moves the cursor up one row, scrolling the region at its top
    /// margin.
    ///
    /// A cursor above a non-zero top margin and already on the first row
    /// moves nothing and scrolls nothing.
    ///
    /// # Control Functions
    ///
    /// - `RI` (`0x8D`, `ESC M`)
    pub fn reverse_index(&mut self) -> Option<DamageSpan> {
        self.state.pending_wrap = false;
        if self.state.line == self.scroll_region.top_margin() {
            return self.scroll_region_down(1);
        }
        if ScreenLine(0) < self.state.line {
            self.state.line.0 -= 1;
        }
        None
    }

    /// Inserts `count` blank rows at the cursor inside the scroll
    /// region: the cursor row and the rows below it move down, the
    /// rows pushed past the bottom margin are lost, and the pen's erase
    /// cell fills the rows that open. The cursor is homed to column
    /// zero and the deferred wrap is disarmed.
    ///
    /// A cursor outside the margins inserts nothing (VT510 "IL — Insert
    /// Line"). The count is clamped to the rows from the cursor through
    /// the bottom margin. An insert never feeds history: the rows it
    /// discards leave from the bottom margin, not the top of the page.
    /// The cursor homing follows xterm, kitty, and ECMA-48 § 8.3.67
    /// rather than alacritty and wezterm, which leave the column alone.
    ///
    /// # Control Functions
    ///
    /// - `IL` (`CSI Pn L`)
    pub fn insert_lines(&mut self, count: u16) -> Option<DamageSpan> {
        if !self.scroll_region.scroll_span().contains(&self.state.line) {
            return None;
        }
        let damage = self.shift_rows_down(self.state.line, count)?;
        self.carriage_return();
        Some(damage)
    }

    /// Deletes `count` rows at the cursor inside the scroll region: the
    /// rows below move up and the pen's erase cell fills the rows that
    /// open at the bottom margin. The cursor is homed to column zero
    /// and the deferred wrap is disarmed.
    ///
    /// A cursor outside the margins deletes nothing (VT510 "DL — Delete
    /// Line"). The count is clamped to the rows from the cursor through
    /// the bottom margin. A delete with the cursor on the first row of
    /// the page feeds the deleted rows to history, as xterm, alacritty,
    /// and wezterm do; the cursor homing follows xterm, kitty, and
    /// ECMA-48 § 8.3.32 rather than alacritty and wezterm, which leave
    /// the column alone.
    ///
    /// # Control Functions
    ///
    /// - `DL` (`CSI Pn M`)
    pub fn delete_lines(&mut self, count: u16) -> Option<DamageSpan> {
        if !self.scroll_region.scroll_span().contains(&self.state.line) {
            return None;
        }
        let damage = self.shift_rows_up(self.state.line, count)?;
        self.carriage_return();
        Some(damage)
    }

    /// Inserts `count` blank characters at the cursor: the cells to its
    /// right move right keeping their own attributes, the cells pushed
    /// past the last column are lost, and the cursor stays where it is.
    ///
    /// The count is clamped to the columns from the cursor through the
    /// last one, and the blanks carry the pen's erase cell. A shift
    /// disarms the deferred wrap; a zero count returns before anything
    /// is touched, that flag included. Unlike [`Self::insert_lines`],
    /// the edit applies wherever the cursor sits, ignoring the
    /// scrolling margins VT510 gates `ICH` on, as xterm, alacritty,
    /// kitty, ghostty, VTE and foot do. Selection and placement
    /// anchors hold absolute columns and do not move with the content.
    ///
    /// # Control Functions
    ///
    /// - `ICH` (`CSI Pn @`)
    /// - `IRM` (`CSI 4 h`) — the shift [`Self::print`] performs for
    ///   each character printed in insert mode
    pub fn insert_characters(&mut self, count: u16) -> Option<DamageSpan> {
        let count = self.clamped_columns(count)?;
        let fill = self.state.pen.erase_cell();
        self.grid
            .insert_visible_row_cells(self.state.line, self.state.column, count, fill);
        self.state.pending_wrap = false;
        self.damage_span(self.state.line, self.state.line)
    }

    /// Deletes `count` characters at the cursor: the cells to their
    /// right move left keeping their own attributes, the pen's erase
    /// cell fills the columns that open at the last column, and the
    /// cursor stays where it is.
    ///
    /// The count is clamped to the columns from the cursor through the
    /// last one, never to the row width, which would blank a column
    /// left of the cursor. Everything [`Self::insert_characters`]
    /// records about the zero count, the scrolling margins, the
    /// deferred wrap, and the column anchors holds here too.
    ///
    /// # Control Functions
    ///
    /// - `DCH` (`CSI Pn P`)
    pub fn delete_characters(&mut self, count: u16) -> Option<DamageSpan> {
        let count = self.clamped_columns(count)?;
        let fill = self.state.pen.erase_cell();
        self.grid
            .delete_visible_row_cells(self.state.line, self.state.column, count, fill);
        self.state.pending_wrap = false;
        self.damage_span(self.state.line, self.state.line)
    }

    /// Scrolls the whole scroll region up by `count` rows: the rows at
    /// the top margin leave and the pen's erase cell fills the rows that
    /// open at the bottom margin. The cursor does not move.
    ///
    /// The count is clamped to the region height. A region whose top
    /// margin is the first row of the page feeds the departing rows to
    /// history, as a line feed there would, and it does so even when a
    /// bottom margin pins content below the region.
    ///
    /// # Control Functions
    ///
    /// - `SU` (`CSI Pn S`)
    pub fn scroll_region_up(&mut self, count: u16) -> Option<DamageSpan> {
        self.shift_rows_up(self.scroll_region.top_margin(), count)
    }

    /// Scrolls the whole scroll region down by `count` rows: the pen's
    /// erase cell fills the rows that open at the top margin and the
    /// rows pushed past the bottom margin are lost. The cursor does not
    /// move.
    ///
    /// The count is clamped to the region height. Nothing is fed to
    /// history.
    ///
    /// # Control Functions
    ///
    /// - `SD` (`CSI Pn T`)
    pub fn scroll_region_down(&mut self, count: u16) -> Option<DamageSpan> {
        self.shift_rows_down(self.scroll_region.top_margin(), count)
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

    /// Shifts the rows from `first` through the bottom margin up by
    /// `count` rows, filling the rows that open at the bottom margin
    /// with the pen's erase cell; `None` when the clamped count is
    /// zero.
    ///
    /// A shift that starts on the first row of the page feeds each
    /// departing row to history and holds a scrolled-back viewport on
    /// the row it was showing, one row at a time, the way
    /// [`Self::line_feed`] does.
    fn shift_rows_up(&mut self, first: ScreenLine, count: u16) -> Option<DamageSpan> {
        let bottom = self.scroll_region.bottom_margin();
        let count = self.clamped_rows(first, count)?;
        let fill = self.state.pen.erase_cell();
        let feeds_history = first == ScreenLine(0);
        for _ in 0..count {
            self.grid.scroll_up_one(first, bottom, fill);
            if feeds_history {
                self.hold_scrolled_viewport();
            }
        }
        Some(DamageSpan::Full)
    }

    /// Shifts the rows from `first` through the bottom margin down by
    /// `count` rows, filling the rows that open at `first` with the
    /// pen's erase cell; `None` when the clamped count is zero.
    ///
    /// The rows pushed past the bottom margin are discarded. Nothing
    /// reaches history on this path, because the rows that leave do so
    /// at the bottom margin rather than at the top of the page.
    fn shift_rows_down(&mut self, first: ScreenLine, count: u16) -> Option<DamageSpan> {
        let bottom = self.scroll_region.bottom_margin();
        let count = self.clamped_rows(first, count)?;
        let fill = self.state.pen.erase_cell();
        for _ in 0..count {
            self.grid.scroll_down_one(first, bottom, fill);
        }
        Some(DamageSpan::Full)
    }

    /// The rows a shift starting at `first` may actually move: `count`
    /// clamped to the rows through the bottom margin, and `None` when
    /// that leaves nothing to do.
    ///
    /// A `first` below the bottom margin also yields `None`, because it
    /// names no row the shift could move. The callers never produce one
    /// — they check the cursor against the margins or pass the top
    /// margin itself — so the guard exists to keep a future caller that
    /// does neither from wrapping the subtraction into a count that
    /// would walk the ring outside the region.
    fn clamped_rows(&self, first: ScreenLine, count: u16) -> Option<u16> {
        let bottom = self.scroll_region.bottom_margin();
        let count = count.min(bottom.0.checked_sub(first.0)? + 1);
        (count > 0).then_some(count)
    }

    /// The columns an in-row edit at the cursor may actually touch:
    /// `count` clamped to the columns from the cursor through the last
    /// one, and `None` when that leaves nothing to do.
    ///
    /// A zero count therefore ends the call before anything is read or
    /// written. The cursor column is always inside the row, so the
    /// subtraction cannot fail; the guard exists to keep a future caller
    /// that seats it outside from wrapping into a count the row cannot
    /// hold.
    fn clamped_columns(&self, count: u16) -> Option<u16> {
        let count = count.min(self.grid.size().cols.checked_sub(self.state.column.0)?);
        (count > 0).then_some(count)
    }

    /// Resolves a motion into the offset it aims at, before clamping.
    ///
    /// A page is a whole screenful with no overlap, and a half page
    /// truncates, so a one-row screen has a zero-sized half page.
    fn scroll_target(&self, scroll: Scroll) -> DisplayOffset {
        let rows = u32::from(self.grid.size().rows);
        let history =
            u32::try_from(self.grid.history_len()).expect("scrollback never exceeds u32::MAX rows");
        let offset = self.viewport.offset.0;
        DisplayOffset(match scroll {
            Scroll::Delta(delta) => offset.saturating_add_signed(delta),
            Scroll::PageUp => offset.saturating_add(rows),
            Scroll::PageDown => offset.saturating_sub(rows),
            Scroll::HalfPageUp => offset.saturating_add(rows / 2),
            Scroll::HalfPageDown => offset.saturating_sub(rows / 2),
            Scroll::Top => history,
            Scroll::Bottom => 0,
        })
    }
}

/// Erasure.
impl Screen {
    /// Erases part of the cursor row with the pen background (BCE);
    /// [`EraseLineMode::ToEnd`] is a no-op while the cursor logically
    /// sits past the row, as [`Self::cursor_parked_past_the_row`]
    /// decides.
    ///
    /// # Control Functions
    ///
    /// - `EL` (`CSI Ps K`)
    pub fn erase_in_line(
        &mut self,
        mode: EraseLineMode,
        auto_wrap: AutoWrap,
    ) -> Option<DamageSpan> {
        if matches!(mode, EraseLineMode::ToEnd) && self.cursor_parked_past_the_row(auto_wrap) {
            return None;
        }
        let cols = self.grid.size().cols;
        let columns = match mode {
            EraseLineMode::ToEnd => self.state.column.0..cols,
            EraseLineMode::ToStart => 0..self.state.column.0 + 1,
            EraseLineMode::All => 0..cols,
        };
        self.erase_cursor_row_columns(columns)
    }

    /// Erases `count` characters from the cursor rightward with the
    /// pen background (BCE), leaving the cursor where it is; a no-op
    /// while the cursor logically sits past the row, as
    /// [`Self::erase_in_line`]'s [`EraseLineMode::ToEnd`] is.
    ///
    /// # Control Functions
    ///
    /// - `ECH` (`CSI Pn X`)
    pub fn erase_chars(&mut self, count: u16, auto_wrap: AutoWrap) -> Option<DamageSpan> {
        if self.cursor_parked_past_the_row(auto_wrap) {
            return None;
        }
        let cols = self.grid.size().cols;
        let start = self.state.column.0;
        let end = start.saturating_add(count).min(cols);
        self.erase_cursor_row_columns(start..end)
    }

    /// Erases part of the visible screen with the pen background
    /// (BCE), in place; scrollback history is never touched.
    ///
    /// # Control Functions
    ///
    /// - `ED` (`CSI Ps J`)
    pub fn erase_in_display(&mut self, mode: EraseScreenMode) -> Option<DamageSpan> {
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
                self.damage_span(self.state.line, ScreenLine(rows - 1))
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
                self.damage_span(ScreenLine(0), self.state.line)
            }
            EraseScreenMode::All => {
                for line in 0..rows {
                    self.grid
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank);
                }
                Some(DamageSpan::Full)
            }
        }
    }

    /// Fills the given column range of the cursor row with the pen's
    /// erase cell and reports that row, which is the whole contract
    /// every single-row erasure shares.
    fn erase_cursor_row_columns(&mut self, columns: Range<u16>) -> Option<DamageSpan> {
        self.grid
            .fill_visible_row_range(self.state.line, columns, self.state.pen.erase_cell());
        self.damage_span(self.state.line, self.state.line)
    }

    /// Whether the cursor logically sits past the row's last cell, so
    /// that an erase from the cursor rightward finds nothing to erase.
    ///
    /// All three conditions are required: the deferred wrap armed,
    /// `DECAWM` set so the next character really does move to the next
    /// row, and the cursor on the last column. The column test is not
    /// redundant — [`Self::tab_to`] carries an armed flag off the right
    /// border, so without it a `CBT` out of a full row would leave
    /// `EL 0` and `ECH` declining mid-row.
    fn cursor_parked_past_the_row(&self, auto_wrap: AutoWrap) -> bool {
        self.state.pending_wrap && auto_wrap.wraps() && self.at_right_edge()
    }

    /// Whether the cursor is on the row's last column.
    fn at_right_edge(&self) -> bool {
        self.state.column.0 + 1 >= self.grid.size().cols
    }
}

/// Tabulation stops.
impl Screen {
    /// Moves the cursor forward `count` tabulation stops.
    ///
    /// The right edge is this screen's last column, so the same stop
    /// table lands the cursor differently on a narrow screen than on a
    /// wide one.
    ///
    /// # Control Functions
    ///
    /// - `HT` (`0x09`) — with a count of one
    /// - `CHT` (`CSI Pn I`)
    pub fn move_forward_tabs(&mut self, count: u16) {
        let right_edge = GridColumn(self.grid_size().cols - 1);
        let target = self.tabs.cht(self.state.column, count, right_edge);
        self.tab_to(target);
    }

    /// Moves the cursor back `count` tabulation stops.
    ///
    /// The left edge is column zero until DECSLRM and DECOM land, at
    /// which point the margin supplies it instead.
    ///
    /// # Control Functions
    ///
    /// - `CBT` (`CSI Pn Z`)
    pub fn move_backward_tabs(&mut self, count: u16) {
        let target = self.tabs.cbt(self.state.column, count, GridColumn(0));
        self.tab_to(target);
    }

    /// Sets a tabulation stop at the cursor column.
    ///
    /// Routed through the same edit vocabulary `CTC 0` uses, because the
    /// two control functions request the identical edit. TABULATION STOP
    /// MODE scoping, when it lands, has to reach HTS as well.
    ///
    /// # Control Functions
    ///
    /// - `HTS` (`0x88`, `ESC H`)
    pub fn set_horizontal_tab_stop(&mut self) {
        self.edit_tab_stop(CharacterTabEdit::SetColumn);
    }

    /// Applies one tabulation stop edit at the cursor column.
    ///
    /// `TBC` and `CTC` number their parameters differently, so the
    /// caller decodes its own parameter space with
    /// [`CharacterTabEdit::from_tbc`] or
    /// [`CharacterTabEdit::from_ctc`] before calling this.
    ///
    /// # Control Functions
    ///
    /// - `TBC` (`CSI Ps g`)
    /// - `CTC` (`CSI Ps W`)
    pub fn edit_tab_stop(&mut self, edit: CharacterTabEdit) {
        let column = self.state.column;
        match edit {
            CharacterTabEdit::SetColumn => self.tabs.set(column),
            CharacterTabEdit::ClearColumn => self.tabs.clear(column),
            CharacterTabEdit::ClearAllColumns => self.tabs.clear_all(),
        }
    }

    /// Reinstalls the default tabulation stride.
    ///
    /// # Control Functions
    ///
    /// - `DECST8C` (`CSI ? 5 W`)
    /// - `RIS` (`ESC c`)
    pub fn reset_tab_stops(&mut self) {
        self.tabs.reset();
    }

    /// Seats the cursor at a tabulation column.
    ///
    /// # Invariants
    ///
    /// The deferred wrap is deliberately left as it is, unlike
    /// [`Screen::carriage_return`]. Disarming it would make a tab after
    /// a full row seat the cursor back onto the row the application had
    /// already filled.
    fn tab_to(&mut self, column: GridColumn) {
        self.state.column = column;
    }
}

/// Graphic character mapping.
impl Screen {
    /// Designates `character_set` to `g_code`.
    ///
    /// The caller decodes the sequence's designator and final character
    /// with [`GCode::from_designator`] and [`CharacterSet::from_dscs`]
    /// before calling this.
    ///
    /// # Control Functions
    ///
    /// - `SCS` (`ESC ( Dscs`, `ESC ) Dscs`, `ESC * Dscs`, `ESC + Dscs`)
    pub fn designate_character_set(&mut self, g_code: GCode, character_set: CharacterSet) {
        self.character_set_mapping.designate(g_code, character_set);
    }

    /// Invokes `g_code` into GL until the next locking shift.
    ///
    /// # Control Functions
    ///
    /// - `LS0` (`SI`, `0x0F`)
    /// - `LS1` (`SO`, `0x0E`)
    /// - `LS2` (`ESC n`)
    /// - `LS3` (`ESC o`)
    pub fn invoke_character_set(&mut self, g_code: GCode) {
        self.character_set_mapping.invoke(g_code);
    }

    /// Invokes `single_shift` into GL for the next graphic character.
    ///
    /// # Control Functions
    ///
    /// - `SS2` (`0x8E`, `ESC N`)
    /// - `SS3` (`0x8F`, `ESC O`)
    pub fn single_shift(&mut self, single_shift: SingleShift) {
        self.character_set_mapping.single_shift(single_shift);
    }
}

/// Graphic rendition.
///
/// The pen is handed out mutably because applying an `SGR` sequence is
/// the caller's job; this screen only supplies the attributes a print
/// stamps into a cell.
impl Screen {
    /// Mutably borrows the SGR pen.
    pub fn pen_mut(&mut self) -> &mut Pen {
        &mut self.state.pen
    }
}

/// Scrolling margins and the cursor origin.
impl Screen {
    /// Sets the scrolling region and seats the cursor at the resulting
    /// home; a request the margins cannot satisfy is refused whole.
    ///
    /// Both parameters are one-based line numbers as sent, with `None`
    /// for an omitted one; [`Margins::resolve`] owns the defaults, the
    /// clamp, and the refusal.
    ///
    /// The cursor goes to the home the origin mode defines rather than
    /// to the "column 1, line 1 of the page" VT510 p.276 states,
    /// because homing to the page while the origin is within the
    /// margins would seat the cursor outside them, which p.195 forbids;
    /// xterm homes through the same origin-aware path.
    ///
    /// # Control Functions
    ///
    /// - `DECSTBM` (`CSI Pt ; Pb r`)
    pub fn set_scroll_region(&mut self, top: Option<u16>, bottom: Option<u16>) {
        let Some(margins) = Margins::resolve(top, bottom, self.grid.size().rows) else {
            return;
        };
        self.scroll_region.set_margins(margins);
        self.seat_home();
    }

    /// Sets the cursor origin and seats the cursor at the home the new
    /// mode defines.
    ///
    /// Both directions seat the cursor. VT510 says only what home *is*
    /// under each setting and never that `DECOM` moves the cursor; xterm,
    /// kitty, wezterm, Windows Terminal, and `vttest` settle it by homing
    /// on set and on reset alike.
    ///
    /// # Control Functions
    ///
    /// - `DECOM` (`CSI ? 6 h` / `CSI ? 6 l`)
    pub fn set_origin_mode(&mut self, origin_mode: OriginMode) {
        self.scroll_region.set_origin_mode(origin_mode);
        self.seat_home();
    }

    /// Seats the cursor at the home the current [`OriginMode`] defines.
    fn seat_home(&mut self) {
        self.seat_cursor(ScreenLine(0), GridColumn(0));
    }
}

/// The viewport the user sees.
impl Screen {
    /// Borrows the cells shown at a viewport line.
    ///
    /// The viewport is the window the user sees: at the live tail it is
    /// the active screen, and a scrolled viewport reaches back into
    /// history. [`crate::screen::grid::Grid`]'s own index resolves
    /// against the live tail alone, so a scrolled read has to come
    /// through here.
    pub fn viewport_row(&self, line: ViewportLine) -> &Row<Cell> {
        self.grid.row(line.to_grid(self.viewport.offset))
    }

    /// Number of scrollback rows the viewport sits above the live tail; always zero until scroll operations arrive.
    #[inline]
    pub const fn display_offset(&self) -> DisplayOffset {
        self.viewport.offset
    }

    /// Moves the viewport by one [`Scroll`] motion; `None` when the
    /// motion was zero or entirely clamped away.
    ///
    /// # Invariants
    ///
    /// A motion that moves the viewport reports [`DamageSpan::Full`]:
    /// the emit-time offset diff only guarantees that a frame is
    /// emitted, not that it carries rows, so anything less would
    /// repaint stale content at the new offset.
    ///
    /// A screen that keeps no history never moves, because every target
    /// clamps to the live tail. That is what makes this a silent no-op
    /// on the alternate screen without a caller having to check.
    pub fn scroll(&mut self, scroll: Scroll) -> Option<DamageSpan> {
        let before = self.viewport.offset;
        self.set_display_offset(self.scroll_target(scroll));
        (self.viewport.offset != before).then_some(DamageSpan::Full)
    }

    /// Seats the viewport at `offset`, clamped to the history that
    /// currently exists.
    ///
    /// [`Self::hold_scrolled_viewport`] also writes the offset, so this is
    /// not the only seam that does; it is the seam a future
    /// `DeviceState::scroll` will drive.
    ///
    /// # Invariants
    ///
    /// The caller must stage full damage: this moves the viewport
    /// basis, so a frame that carried the new offset without every row
    /// would repaint stale content.
    pub fn set_display_offset(&mut self, offset: DisplayOffset) {
        let history =
            u32::try_from(self.grid.history_len()).expect("scrollback never exceeds u32::MAX rows");
        self.viewport.offset = DisplayOffset(offset.0.min(history));
    }
}

/// What an emitted frame reads back.
///
/// The damage projection lives here because it converts screen rows into
/// the viewport coordinates a frame repaints by.
impl Screen {
    /// Returns the grid size.
    pub fn grid_size(&self) -> GridSize {
        self.grid.size()
    }

    /// The write cursor as an emitted frame carries it.
    ///
    /// `text_cursor_enable` is `DECTCEM`, which the device owns rather
    /// than either screen. Production callers reach this through
    /// `DeviceState::cursor`, which records why pairing a screen read
    /// with a separately-read mode silently drops a `CSI ? 25 l`.
    // TODO: Report the real shape and blink once DECSCUSR lands. Block /
    // steady is what the terminal starts at.
    pub fn cursor(&self, text_cursor_enable: TextCursorEnable) -> Cursor {
        Cursor {
            point: GridPoint {
                line: GridLine::from(self.state.line),
                column: self.state.column,
            },
            shape: CursorShape::Block,
            blinking: false,
            visible: matches!(text_cursor_enable, TextCursorEnable::Shown),
        }
    }

    /// The cursor position as a `CSI 6 n` report carries it: 1-based,
    /// and relative to the top margin while origin mode confines the
    /// cursor to the scroll region.
    pub fn cursor_position_report(&self) -> (u16, u16) {
        let origin = match self.scroll_region.origin_mode() {
            OriginMode::WithinMargins => self.scroll_region.top_margin(),
            OriginMode::UpperLeftCorner => ScreenLine(0),
        };
        let row = self.state.line.0.saturating_sub(origin.0) + 1;
        let column = self.state.column.0 + 1;
        (row, column)
    }

    /// The selection as an emitted frame carries it: normalized,
    /// cell-side trimmed, in active-grid coordinates; `None` when there
    /// is no selection, its span is empty, or an endpoint's row has
    /// left the ring.
    pub fn selection_range(&self) -> Option<SelectionRange> {
        match self
            .selection
            .resolve(|id| self.grid.grid_line(id), self.grid.size().cols)
        {
            Resolved::Range(range) => Some(range),
            Resolved::None | Resolved::Empty => None,
        }
    }

    /// The id of the row the cursor sits on — the anchor a mount samples.
    pub fn cursor_line_id(&self) -> LineId {
        self.grid.line_id(self.state.line)
    }

    /// The cursor's column.
    pub fn cursor_column(&self) -> GridColumn {
        self.state.column
    }

    /// Reports the given screen rows as damage, in the viewport
    /// coordinates a frame repaints by; `None` when the whole span has
    /// scrolled out of the window.
    fn damage_span(&self, first: ScreenLine, last: ScreenLine) -> Option<DamageSpan> {
        debug_assert!(first <= last, "a damage span runs top to bottom");
        let rows = self.grid.size().rows;
        // NOTE: `DisplayOffset` is a `u32` and does not bound itself, so the
        // shift has to saturate — a wrapping add would report an off-screen
        // row as visible.
        let offset = self.viewport.offset.0;
        let first = u32::from(first.0).saturating_add(offset);
        if first >= u32::from(rows) {
            return None;
        }
        let last = u32::from(last.0)
            .saturating_add(offset)
            .min(u32::from(rows - 1));
        Some(DamageSpan::rows(
            ViewportLine(u16::try_from(first).expect("guarded above by first < rows")),
            ViewportLine(u16::try_from(last).expect("clamped to rows - 1 above")),
        ))
    }
}

/// The state DECSC copies aside.
impl Screen {
    /// Saves the current state in memory in accordance with DECSC.
    ///
    /// # Control Functions
    ///
    /// - DECSC(Save Cursor)
    pub fn save_checkpoint(&mut self) {
        self.checkpoint = self.capture_checkpoint();
    }

    /// Applies the state saved in memory to each actual state.
    /// If no saved state exists, perform a DECRC-compliant action.
    ///
    /// The saved position is put back verbatim. Restoring an origin mode
    /// whose margins moved in between can therefore seat the cursor
    /// outside them; DECSC saves no margins to clamp against, and the
    /// manuals leave the collision undefined.
    ///
    /// # Control Functions
    ///
    /// - DECRC(Restore Cursor)
    pub fn restore_checkpoint(&mut self) {
        let saved = self.checkpoint;
        self.state.line = saved.line;
        self.state.column = saved.column;
        self.state.pen = saved.pen;
        self.state.pending_wrap = saved.pending_wrap;
        self.scroll_region.set_origin_mode(saved.origin_mode);
        self.character_set_mapping = saved.character_set_mapping;
    }

    fn capture_checkpoint(&self) -> Checkpoint {
        Checkpoint::capture(
            &self.state,
            self.scroll_region.origin_mode(),
            self.character_set_mapping,
        )
    }
}

/// Whole-screen state replacement.
impl Screen {
    /// Resets the screen to its power-up state.
    ///
    /// Covers the screen-scoped actions of `RIS`: the grid and its
    /// history, the cursor, the SGR pen, the scrolling margins, the
    /// origin mode, the tabulation stops, and the character set
    /// mapping.
    ///
    /// Reports [`DamageSpan::Full`], or nothing when the grid was
    /// already blank and carried no history; the cursor homes either
    /// way, because cursor motion reaches the renderer through the
    /// per-chunk cursor diff rather than through damage. A selection the
    /// reset drops also reports `Full`, so the frame that no longer
    /// carries it is owed even on a blank grid.
    ///
    /// # Invariants
    ///
    /// The cursor lands at the screen's upper-left corner whatever
    /// origin mode was in force, because the state is replaced wholesale
    /// rather than homed through the origin.
    ///
    /// Every placement on this screen becomes evictable here without
    /// this method touching the table: [`crate::screen::grid::Grid::reset`]
    /// mints fresh row ids without rewinding its counter, so no anchor
    /// taken before the reset can resolve afterwards and the next
    /// [`Self::evict_lost_anchors`] names all of them. A rewrite of
    /// `Grid::reset` that renumbers from zero would silently keep the
    /// placements alive.
    ///
    /// # Control Functions
    ///
    /// - `RIS` (`ESC c`) — its screen-scoped actions
    pub fn reset(&mut self) -> Option<DamageSpan> {
        let cleared = self.selection.clear();
        let dirty = !self.grid.is_blank() || cleared;
        self.grid.reset();
        self.viewport = Viewport::default();
        self.scroll_region = ScrollRegion::new(self.grid.size().rows);
        self.state = ScreenState::default();
        self.tabs = TabStops::default();
        self.character_set_mapping = CharacterSetMapping::default();
        self.checkpoint = Checkpoint::default();
        dirty.then_some(DamageSpan::Full)
    }

    /// Resizes the grid, truncating rather than reflowing; `None` when
    /// the dimensions already matched.
    ///
    /// A shrink pushes as many rows off the top as it takes to keep the
    /// cursor on screen and drops the rest from the bottom, so a prompt
    /// at the bottom survives and a mostly-blank screen keeps its
    /// content. A growth reclaims rows from history before it appends
    /// blank ones.
    ///
    /// # Invariants
    ///
    /// A resize that changes the dimensions reports [`DamageSpan::Full`]:
    /// every emitted frame carries the new size but nothing diffs it, so
    /// partial row damage would hand the renderer new dimensions with
    /// stale rows behind them.
    ///
    /// The cursor and the saved cursor both land inside the new grid.
    /// Leaving either out of bounds would panic the next write, which is
    /// why the saved one is clamped here rather than on restore.
    ///
    /// The saved cursor follows the rows a resize moves exactly as the
    /// live one does, so a `DECRC` after the resize — the one
    /// `DECRST 1049` performs on the way back from the alternate screen
    /// included — lands on the row `DECSC` saved rather than the rows
    /// the resize reclaimed above it. A never-saved checkpoint drifts
    /// off the home position by the same amount, the trade alacritty
    /// makes too.
    ///
    /// A height change returns the margins to the whole page. Keeping a
    /// region whose rows still fit would leave a cursor below its bottom
    /// margin, and [`Self::line_feed`] scrolls only on an exact match
    /// with that margin, so the screen would never scroll again.
    ///
    /// A scrolled-back viewport tracks the rows it was showing: each
    /// scroll pairs with [`Self::hold_scrolled_viewport`] the way line
    /// feeding does, and a growth walks the offset back by the rows it
    /// reclaims.
    pub fn resize(&mut self, size: GridSize) -> Option<DamageSpan> {
        let old = self.grid.size();
        if old == size {
            return None;
        }
        let required_scrolling = (self.state.line.0 + 1).saturating_sub(size.rows);
        for _ in 0..required_scrolling {
            self.grid
                .scroll_up_one(ScreenLine(0), ScreenLine(old.rows - 1), Cell::default());
            self.hold_scrolled_viewport();
        }
        let reclaimed = self.reclaimable_rows(old.rows, size.rows);
        self.grid.resize(size);
        self.shift_cursors(reclaimed, required_scrolling);
        self.clamp_cursors(size);
        if old.cols != size.cols {
            self.state.pending_wrap = false;
            self.checkpoint.pending_wrap = false;
        }
        if old.rows != size.rows {
            self.scroll_region.set_margins(Margins::new(size.rows));
        }
        self.set_display_offset(DisplayOffset(
            self.viewport.offset.0.saturating_sub(u32::from(reclaimed)),
        ));
        Some(DamageSpan::Full)
    }

    /// Fills the visible screen with the alignment pattern, returning to
    /// the page-wide scroll region and the absolute cursor origin.
    ///
    /// The pattern is drawn with default attributes rather than the
    /// current pen, because a screen tinted by the application's colors
    /// is useless as an adjustment reference.
    ///
    /// # Control Functions
    ///
    /// - `DECALN` (`ESC # 8`)
    pub fn fill_alignment_pattern(&mut self) -> DamageSpan {
        let size = self.grid.size();
        let cell = Cell {
            c: 'E',
            ..Cell::default()
        };
        for line in 0..size.rows {
            self.grid
                .fill_visible_row_range(ScreenLine(line), 0..size.cols, cell);
        }
        self.scroll_region = ScrollRegion::new(size.rows);
        self.seat_cursor(ScreenLine(0), GridColumn(0));
        DamageSpan::Full
    }

    fn reclaimable_rows(&self, old_rows: u16, rows: u16) -> u16 {
        if rows <= old_rows {
            return 0;
        }
        let reclaimed = usize::from(rows - old_rows).min(self.grid.history_len());
        u16::try_from(reclaimed).expect("a growth never exceeds u16::MAX rows")
    }

    /// Moves the live cursor and the saved one down by the rows a resize
    /// reclaimed from history and up by the rows it scrolled away, so
    /// both keep pointing at the row they were on.
    fn shift_cursors(&mut self, reclaimed: u16, required_scrolling: u16) {
        let follow_moved_rows = |line: &mut ScreenLine| {
            line.0 = line
                .0
                .saturating_add(reclaimed)
                .saturating_sub(required_scrolling);
        };
        follow_moved_rows(&mut self.state.line);
        follow_moved_rows(&mut self.checkpoint.line);
    }

    fn clamp_cursors(&mut self, size: GridSize) {
        self.state.line.0 = self.state.line.0.min(size.rows - 1);
        self.state.column.0 = self.state.column.0.min(size.cols - 1);
        self.checkpoint.line.0 = self.checkpoint.line.0.min(size.rows - 1);
        self.checkpoint.column.0 = self.checkpoint.column.0.min(size.cols - 1);
    }
}

/// Webview placements.
///
/// The anchor a mount records is a `LineId` from this screen's own grid,
/// so a placement can only ever be resolved against the grid that minted
/// its anchor.
///
/// Five of these forward to [`ScreenPlacements`] unchanged. They stay
/// rather than exposing the table, so `DeviceState` never holds a
/// `&mut ScreenPlacements` and every mutation of a screen's placements
/// goes through the screen that owns them.
impl Screen {
    /// Registers a mount at the write cursor under the id the host minted.
    pub fn mount_placement(&mut self, id: InstanceId, size: PlacementSize) {
        let anchor = self.cursor_line_id();
        let col = self.cursor_column();
        self.placements.mount(id, anchor, col, size);
    }

    /// Registers a mount anchored at the visible row `row` and column
    /// `column` under the id the host minted.
    ///
    /// # Invariants
    ///
    /// `row` and `column` lie inside the grid: `Grid::line_id` indexes the
    /// ring unchecked, so the device bounds-checks against `grid_size`
    /// before calling this.
    pub fn mount_placement_at(
        &mut self,
        id: InstanceId,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
    ) {
        let anchor = self.grid.line_id(row);
        self.placements.mount(id, anchor, column, size);
    }

    /// Drops the placement a re-mount replaces, without reporting it.
    pub fn supersede_placement(&mut self, id: InstanceId) {
        self.placements.supersede(id);
    }

    /// Removes the placement a client `unmount` addresses (`None`
    /// removes every placement on this screen); returns whether
    /// anything went.
    pub fn unmount_placement(&mut self, id: Option<InstanceId>) -> bool {
        self.placements.unmount(id)
    }

    /// Removes the placements the host names; returns whether anything
    /// went.
    pub fn remove_placements(&mut self, ids: &[InstanceId]) -> bool {
        self.placements.remove_many(ids)
    }

    /// Empties this screen's table and names every id it held.
    pub fn take_placements(&mut self) -> Vec<InstanceId> {
        self.placements.take_all()
    }

    /// Number of placements this screen holds.
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Resolves this screen's placements into grid coordinates.
    ///
    /// # Invariants
    ///
    /// The anchors resolve through the same expression
    /// [`Self::evict_lost_anchors`] passes. A placement this omits is
    /// exactly a placement the sweep evicts, so no placement can become
    /// unresolvable without also becoming evictable.
    pub fn project_placements(&self) -> Vec<AnchoredPlacement> {
        self.placements
            .project(|anchor| self.grid.grid_line(anchor))
    }

    /// Drops the placements whose anchor row left this screen's grid and
    /// names them.
    pub fn evict_lost_anchors(&mut self) -> Vec<InstanceId> {
        self.placements
            .evict_lost_anchors(|anchor| self.grid.grid_line(anchor))
    }
}

/// The selection this screen owns.
///
/// The endpoints are resolved through the same expression
/// [`Self::project_placements`] passes for anchors, so a selection can
/// only ever be resolved against the grid that minted its rows.
impl Screen {
    /// Anchors a new selection at `cell`, replacing any active one;
    /// returns whether the state changed. A cell outside the grid is
    /// rejected and leaves the current selection untouched.
    pub fn start_selection(
        &mut self,
        cell: GridPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> bool {
        let Some(end) = self.selection_end(cell, side) else {
            return false;
        };
        self.selection.start(end, kind)
    }

    /// Moves the active selection's moving end to `cell`; returns
    /// whether it moved. A no-op without an active selection or for a
    /// cell outside the grid.
    pub fn extend_selection(&mut self, cell: GridPoint, side: CellSide) -> bool {
        let Some(end) = self.selection_end(cell, side) else {
            return false;
        };
        self.selection.extend(end)
    }

    /// Drops the active selection; returns whether there was one, even
    /// one whose rows have already left the ring.
    pub fn clear_selection(&mut self) -> bool {
        self.selection.clear()
    }

    /// The text the active selection covers, row by row; `None` exactly
    /// when [`Self::selection_range`] is `None`.
    ///
    /// Each row's span comes from [`SelectionRange::span_on`], with
    /// trailing blanks trimmed. Rows are joined by `\n` with none after
    /// the last.
    // TODO: Join soft-wrapped rows without a newline once `Row` records
    // the wrap.
    pub fn selection_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        let last_column = self.grid.size().cols - 1;
        let mut text = String::new();
        for line in range.start.line.0..=range.end.line.0 {
            let (first, last) = range.span_on(line, last_column);
            let row = self.grid.row(GridLine(line));
            let row_text: String = (first..=last)
                .map(|column| row[GridColumn(column)].c)
                .collect();
            if line != range.start.line.0 {
                text.push('\n');
            }
            text.push_str(row_text.trim_end());
        }
        Some(text)
    }

    /// The endpoint a host cell stands for; `None` when the cell is
    /// outside the ring or past the width.
    fn selection_end(&self, cell: GridPoint, side: CellSide) -> Option<SelectionEnd> {
        let line = self.grid.line_id_at_point(cell)?;
        Some(SelectionEnd::at(line, cell.column, side))
    }
}

#[cfg(test)]
mod tests;
