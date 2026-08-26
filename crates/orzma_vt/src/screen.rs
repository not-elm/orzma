//! The atomic grid + cursor operation unit for one terminal screen.
//!
//! [`Screen`] owns cell storage ([`grid::Grid`]) and the write cursor,
//! and updates them together; a mutation that damages rows returns the
//! [`DamageSpan`] it produced for the caller to stage, and pure cursor
//! motion returns nothing, because the per-chunk cursor diff reports
//! it.

// TODO: This attribute is file-level, so it silences dead_code across
// the whole `screen/` subtree (12 files, ~5,400 lines), though only
// about 8 items actually need it. Narrow it to item-level `#[expect]`s
// once the executor's CSI handlers land and most of those items go
// live.
// NOTE: the `#[cfg(test)]` module below uses every item this lint
// would flag, so an unconditional `#[expect(dead_code)]` is fulfilled
// in a plain build but unfulfilled — and denied under `-D warnings` —
// in a test build. Gating it to non-test builds keeps both clean.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the executor reaches these screen operations once its CSI handlers land"
    )
)]

pub mod cell;
pub mod character_sets;
pub mod checkpoint;
pub mod grid;
pub mod margins;
pub mod tabs;
pub mod viewport;

pub(crate) mod cursor;

mod state;

use self::cell::{Cell, Pen};
use self::grid::Grid;
use self::grid::LineId;
use self::grid::row::Row;
use crate::frame::damage::DamageSpan;
use crate::screen::character_sets::{
    CharacterSet, CharacterSetMapping, GCode, GraphicChar, SingleShift,
};
use crate::screen::checkpoint::Checkpoint;
use crate::screen::cursor::{Cursor, CursorShape};
use crate::screen::grid::GridSize;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
use crate::screen::margins::{Margins, OriginMode, ScrollRegion};
use crate::screen::state::ScreenState;
use crate::screen::tabs::{CharacterTabEdit, TabStops};
use crate::screen::viewport::{DisplayOffset, Viewport, ViewportLine};

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
        }
    }
}

/// Graphic character output.
impl Screen {
    /// Prints one character at the cursor with the current pen,
    /// wrapping first when the deferred wrap is armed.
    ///
    /// The caller dispatches control bytes itself; this method assumes
    /// a printable character of display width one.
    ///
    /// A wrap that scrolled reports [`DamageSpan::Full`]; every other print
    /// reports the row the character landed on, or nothing when that row has
    /// scrolled out of the window. [`Self::line_feed`] reports nothing for
    /// the wrap's cursor motion, so passing its value through would leave
    /// the character just written unpainted.
    pub fn print(&mut self, c: char) -> Option<DamageSpan> {
        let GraphicChar(glyph) = self.character_set_mapping.translate(c);
        let wrap = if self.state.pending_wrap {
            self.state.pending_wrap = false;
            self.state.column = GridColumn(0);
            self.line_feed()
        } else {
            None
        };
        self.grid[self.state.line][self.state.column] = self.state.pen.stamp(glyph);
        if self.state.column.0 + 1 < self.grid.size().cols {
            self.state.column.0 += 1;
        } else {
            self.state.pending_wrap = true;
        }
        match wrap {
            Some(DamageSpan::Full) => Some(DamageSpan::Full),
            _ => self.damage_span(self.state.line, self.state.line),
        }
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
        self.state.column = GridColumn(self.state.column.0.saturating_sub(1));
        self.state.pending_wrap = false;
    }

    /// Rewinds the cursor to column zero and disarms the deferred wrap.
    ///
    /// # Control Functions
    ///
    /// - `CR` (`0x0D`)
    /// - `NEL` (`0x85`, `ESC E`) — its first half
    pub fn carriage_return(&mut self) {
        self.state.column = GridColumn(0);
        self.state.pending_wrap = false;
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
        let line = match line {
            None | Some(0) => 1,
            Some(value) => value,
        };
        let column = match column {
            None | Some(0) => 1,
            Some(value) => value,
        };
        self.seat_cursor(ScreenLine(line - 1), GridColumn(column - 1));
    }

    /// Seats the cursor at `line` — measured from the origin the current
    /// [`OriginMode`] defines — and `column`, clamping both axes and
    /// disarming the deferred wrap. The disarm follows xterm, whose
    /// `CursorSet` ends in `ResetWrap`, unlike a linefeed, which
    /// preserves the wrap on purpose.
    ///
    /// Every control function that addresses the cursor ends here, so
    /// the origin, the clamps, and the wrap are decided in one place and
    /// cannot drift between them.
    fn seat_cursor(&mut self, line: ScreenLine, column: GridColumn) {
        let GridSize { cols, rows } = self.grid.size();
        let (origin, last) = match self.scroll_region.origin_mode() {
            OriginMode::WithinMargins => (
                self.scroll_region.top_margin(),
                self.scroll_region.bottom_margin(),
            ),
            OriginMode::UpperLeftCorner => (ScreenLine(0), ScreenLine(rows - 1)),
        };
        self.state.line = ScreenLine(line.0.saturating_add(origin.0).min(last.0));
        self.state.column = GridColumn(column.0.min(cols - 1));
        self.state.pending_wrap = false;
    }
}

/// Line feeding and region scrolling.
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
            let top = self.scroll_region.top_margin();
            self.grid.scroll_up_one(
                top,
                self.scroll_region.bottom_margin(),
                self.state.pen.erase_cell(),
            );
            if top == ScreenLine(0) {
                self.hold_scrolled_viewport();
            }
            return Some(DamageSpan::Full);
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
            self.grid.scroll_down_one(
                self.scroll_region.top_margin(),
                self.scroll_region.bottom_margin(),
                self.state.pen.erase_cell(),
            );
            return Some(DamageSpan::Full);
        }
        if ScreenLine(0) < self.state.line {
            self.state.line.0 -= 1;
        }
        None
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
}

/// Erasure.
impl Screen {
    /// Erases part of the cursor row with the pen background (BCE);
    /// [`EraseLineMode::ToEnd`] is a no-op while the deferred wrap is
    /// armed.
    ///
    /// # Control Functions
    ///
    /// - `EL` (`CSI Ps K`)
    pub fn erase_in_line(&mut self, mode: EraseLineMode) -> Option<DamageSpan> {
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
        self.damage_span(self.state.line, self.state.line)
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
    /// Mutably borrows the SGR pen; applying SGR sequences is the
    /// caller's job.
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
        let offset =
            i32::try_from(self.viewport.offset.0).expect("scrollback never exceeds i32::MAX rows");
        self.grid.row(GridLine(i32::from(line.0) - offset))
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
}

/// What an emitted frame reads back.
///
/// The damage projection lives here because it converts screen rows into
/// the viewport coordinates a frame repaints by, which is the same
/// coordinate space the readbacks above report in.
impl Screen {
    /// Returns the grid size.
    pub fn grid_size(&self) -> GridSize {
        self.grid.size()
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

/// Screen-wide state operations.
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
    /// per-chunk cursor diff rather than through damage.
    ///
    /// # Invariants
    ///
    /// The cursor lands at the screen's upper-left corner whatever
    /// origin mode was in force, because the state is replaced wholesale
    /// rather than homed through the origin.
    ///
    /// # Control Functions
    ///
    /// - `RIS` (`ESC c`) — its screen-scoped actions
    pub fn reset(&mut self) -> Option<DamageSpan> {
        let dirty = !self.grid.is_blank();
        self.grid.reset();
        self.viewport = Viewport::default();
        self.scroll_region = ScrollRegion::new(self.grid.size().rows);
        self.state = ScreenState::default();
        self.tabs = TabStops::default();
        self.character_set_mapping = CharacterSetMapping::default();
        self.checkpoint = Checkpoint::default();
        dirty.then_some(DamageSpan::Full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::Color;
    use crate::screen::margins::Margins;

    fn screen() -> Screen {
        Screen::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Twenty columns put the right edge at 19, so the default stride's
    /// stops at 8 and 16 are reachable and the one at 24 is not.
    fn wide_screen() -> Screen {
        Screen::new(GridSize { cols: 20, rows: 3 }, 10)
    }

    /// Four rows leave two rows below a bottom margin at row 1, so a
    /// cursor outside the region has somewhere left to move down to.
    fn tall_screen() -> Screen {
        Screen::new(GridSize { cols: 4, rows: 4 }, 10)
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
            assert_eq!(
                damage,
                Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
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
            assert_eq!(
                damage,
                Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))
            );
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
            screen.line_feed();
            screen.set_display_offset(DisplayOffset(1));
            screen.state.line = ScreenLine(0);
            assert_eq!(
                screen.print('x'),
                Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))
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
            assert_eq!(damage, Some(DamageSpan::Full));
        }

        /// Asserts that a write below the bottom of the scrolled window
        /// reports no damage at all.
        ///
        /// Case: the user reads scrollback while a build keeps printing at
        /// the live tail, which the window no longer shows.
        #[test]
        fn a_write_scrolled_out_of_the_window_reports_no_damage() {
            let mut screen = screen();
            for _ in 0..3 {
                screen.state.line = ScreenLine(2);
                screen.line_feed();
            }
            screen.viewport.offset = DisplayOffset(3);
            screen.state.line = ScreenLine(0);
            assert_eq!(screen.print('x'), None);
        }
    }

    mod backspace {
        use super::*;

        /// Asserts that a backspace steps the cursor one column left.
        ///
        /// Case: a shell line editor erases the character the user just
        /// typed, moving left before overwriting it with a space.
        #[test]
        fn backspace_moves_the_cursor_one_column_left() {
            let mut screen = screen();
            screen.state.column = GridColumn(2);
            screen.backspace();
            assert_eq!(screen.state.column, GridColumn(1));
        }

        /// Asserts that a backspace at column zero leaves the cursor
        /// where it is rather than wrapping back onto the previous row.
        ///
        /// Case: a program emits more backspaces than it printed
        /// characters, running past the start of the line.
        #[test]
        fn a_backspace_at_column_zero_does_not_move() {
            let mut screen = screen();
            screen.backspace();
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
            screen.backspace();
            assert_eq!(screen.state.column, GridColumn(2));
            assert!(!screen.state.pending_wrap);
        }

        /// Asserts that a backspace at column zero still disarms a
        /// pending deferred wrap.
        ///
        /// Case: a one-column screen prints a character, which arms the
        /// wrap without ever leaving column zero, and the application
        /// then emits a backspace.
        #[test]
        fn a_backspace_at_column_zero_disarms_a_pending_wrap() {
            let mut screen = Screen::new(GridSize { cols: 1, rows: 3 }, 10);
            screen.print('x');
            assert!(screen.state.pending_wrap);
            screen.backspace();
            assert_eq!(screen.state.column, GridColumn(0));
            assert!(!screen.state.pending_wrap);
        }
    }

    mod carriage_return {
        use super::*;

        /// Asserts that a carriage return rewinds the column and clears
        /// the deferred-wrap flag.
        ///
        /// Case: a shell prints a partial line and returns to overwrite it,
        /// as progress indicators do with a bare `\r`.
        #[test]
        fn carriage_return_rewinds_and_clears_pending_wrap() {
            let mut screen = screen();
            screen.state.column = GridColumn(2);
            screen.state.pending_wrap = true;
            screen.carriage_return();
            assert_eq!(screen.state.column, GridColumn(0));
            assert!(!screen.state.pending_wrap);
        }

        /// Asserts that a carriage return at column zero still disarms
        /// a pending deferred wrap.
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
            screen.carriage_return();
            assert!(!screen.state.pending_wrap);
        }
    }

    mod tab_to {
        use super::*;

        /// Asserts that a tab seats the cursor at the target column.
        ///
        /// Case: the shell emits a tab while listing a directory in
        /// aligned columns.
        #[test]
        fn a_tab_seats_the_cursor_at_the_target_column() {
            let mut screen = screen();
            screen.tab_to(GridColumn(2));
            assert_eq!(screen.state.column, GridColumn(2));
        }

        /// Asserts that seating the cursor leaves an armed deferred
        /// wrap alone.
        ///
        /// The agreed policy preserves the flag, unlike
        /// [`Screen::carriage_return`]. Disarming it would seat the
        /// cursor back onto the row the application had already filled,
        /// which is the behaviour both VTE and Windows Terminal found
        /// real DEC hardware never had.
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

    mod move_forward_tabs {
        use super::*;

        /// Asserts that a tab seats the cursor on the next stop.
        ///
        /// Case: the shell emits a tab at the start of a line while
        /// printing aligned columns.
        #[test]
        fn ht_moves_to_the_next_stop() {
            let mut screen = wide_screen();
            screen.move_forward_tabs(1);
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
            screen.move_forward_tabs(1);
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
            screen.move_forward_tabs(1);
            assert_eq!(screen.state.column, GridColumn(3));
        }

        /// Asserts that a counted forward tab skips the stops in
        /// between.
        ///
        /// Case: an application emits `CSI 2 I` to jump two tab
        /// positions in one step.
        #[test]
        fn cht_counts_multiple_stops() {
            let mut screen = wide_screen();
            screen.move_forward_tabs(2);
            assert_eq!(screen.state.column, GridColumn(16));
        }
    }

    mod move_backward_tabs {
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
            screen.move_backward_tabs(1);
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
            screen.move_backward_tabs(1);
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
            screen.set_horizontal_tab_stop();
            screen.state.column = GridColumn(0);
            screen.move_forward_tabs(1);
            assert_eq!(screen.state.column, GridColumn(3));
        }

        /// Asserts that setting a stop leaves the cursor where it was.
        ///
        /// HTS edits the stop table and nothing else; the neighbouring
        /// name HT is the one that moves. Nothing on screen changes
        /// either, which is why `set_horizontal_tab_stop` reports no
        /// damage to stage.
        ///
        /// Case: an application installs a tab position at the column it
        /// is already writing at, then keeps printing on the same line.
        #[test]
        fn hts_does_not_move_the_cursor() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(3);
            screen.set_horizontal_tab_stop();
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
            by_hts.set_horizontal_tab_stop();

            let mut by_ctc = wide_screen();
            by_ctc.state.column = GridColumn(3);
            by_ctc.edit_tab_stop(CharacterTabEdit::from_ctc(0).unwrap());

            for screen in [&mut by_hts, &mut by_ctc] {
                screen.state.column = GridColumn(0);
                screen.move_forward_tabs(1);
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
            screen.edit_tab_stop(CharacterTabEdit::from_tbc(0).unwrap());
            screen.state.column = GridColumn(0);
            screen.move_forward_tabs(1);
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
            screen.edit_tab_stop(CharacterTabEdit::from_tbc(3).unwrap());
            screen.move_forward_tabs(1);
            assert_eq!(screen.state.column, GridColumn(19));
        }

        /// Asserts that CTC sets and clears the stop under the cursor.
        ///
        /// Case: an application uses CTC rather than HTS and TBC to edit
        /// the tab position it is parked on.
        #[test]
        fn ctc_zero_sets_and_ctc_two_clears_at_the_cursor() {
            let mut screen = wide_screen();
            screen.state.column = GridColumn(3);
            screen.edit_tab_stop(CharacterTabEdit::from_ctc(0).unwrap());
            screen.state.column = GridColumn(0);
            screen.move_forward_tabs(1);
            assert_eq!(screen.state.column, GridColumn(3));

            screen.edit_tab_stop(CharacterTabEdit::from_ctc(2).unwrap());
            screen.state.column = GridColumn(0);
            screen.move_forward_tabs(1);
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
            screen.edit_tab_stop(CharacterTabEdit::from_tbc(3).unwrap());
            screen.reset_tab_stops();
            screen.move_forward_tabs(1);
            assert_eq!(screen.state.column, GridColumn(8));
        }
    }

    mod line_feed {
        use super::*;

        /// Asserts that a linefeed above the bottom row only moves the
        /// cursor and reports no damage.
        ///
        /// Case: a shell prints multiple output lines while the screen
        /// still has empty rows below the cursor.
        #[test]
        fn a_linefeed_above_the_bottom_moves_the_cursor() {
            let mut screen = screen();
            let damage = screen.line_feed();
            assert_eq!(screen.state.line, ScreenLine(1));
            assert_eq!(damage, None);
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
            assert_eq!(screen.line_feed(), Some(DamageSpan::Full));
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
            screen.line_feed();
            assert_eq!(screen.grid[ScreenLine(2)][0].bg, Color::Indexed(4));
            assert_eq!(screen.grid[ScreenLine(2)][3].bg, Color::Indexed(4));
        }

        /// Asserts that a linefeed below the bottom margin moves the
        /// cursor down and scrolls nothing.
        ///
        /// The agreed policy gates the scroll on the cursor sitting
        /// exactly at the bottom margin, the way VT510 writes IND and
        /// NEL, rather than on the cursor having reached it: a cursor
        /// outside the region moves like an ordinary cursor-down instead
        /// of scrolling rows it is not among.
        ///
        /// Case: an application reserves a two-row footer below its
        /// scrolling pane and emits a linefeed while the cursor rests on
        /// the footer's first row.
        #[test]
        fn a_linefeed_below_a_bottom_margin_moves_the_cursor_down() {
            let mut screen = tall_screen();
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(0),
                bottom: ScreenLine(1),
            });
            screen.state.line = ScreenLine(2);
            let damage = screen.line_feed();
            assert_eq!(screen.state.line, ScreenLine(3));
            assert_eq!(screen.grid.history_len(), 0);
            assert_eq!(damage, None);
        }

        /// Asserts that a linefeed below the bottom margin, already on
        /// the last row, moves and scrolls nothing.
        ///
        /// The agreed policy mirrors the reverse index above a top
        /// margin: a cursor that hits the screen edge outside the
        /// scrolling region stays put, rather than scrolling the region
        /// it is not inside.
        ///
        /// Case: an application reserves a footer below its scrolling
        /// pane and emits a linefeed while the cursor rests on the last
        /// row of that footer.
        #[test]
        fn a_linefeed_below_a_bottom_margin_at_the_last_row_does_nothing() {
            let mut screen = tall_screen();
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(0),
                bottom: ScreenLine(1),
            });
            screen.grid[ScreenLine(0)][0].c = 'a';
            screen.state.line = ScreenLine(3);
            let damage = screen.line_feed();
            assert_eq!(screen.state.line, ScreenLine(3));
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(screen.grid.history_len(), 0);
            assert_eq!(damage, None);
        }

        /// Asserts that a linefeed at the bottom of a region below a
        /// non-zero top margin rotates the region and leaves history and
        /// the rows above it alone.
        ///
        /// The agreed policy feeds scrollback only when the top margin is
        /// row zero, following alacritty: rows leaving a region that has
        /// content pinned above it never reached the top of the screen,
        /// so treating them as scrollback would interleave them with
        /// output the user never scrolled past.
        ///
        /// Case: an application pins a header on the first row and
        /// scrolls the pane below it forward.
        #[test]
        fn a_linefeed_below_a_top_margin_rotates_without_feeding_history() {
            let mut screen = tall_screen();
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(1),
                bottom: ScreenLine(3),
            });
            for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c'), (3, 'd')] {
                screen.grid[ScreenLine(line)][0].c = glyph;
            }
            screen.state.line = ScreenLine(3);
            let blank = screen.state.pen.erase_cell().c;
            screen.line_feed();
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, 'c');
            assert_eq!(screen.grid[ScreenLine(2)][0].c, 'd');
            assert_eq!(screen.grid[ScreenLine(3)][0].c, blank);
            assert_eq!(screen.grid.history_len(), 0);
        }

        /// Asserts that a linefeed at a bottom margin above the last row
        /// still feeds history and leaves the rows below the margin
        /// standing.
        ///
        /// Case: an application keeps a status line on the last row and
        /// scrolls the pane above it forward.
        #[test]
        fn a_linefeed_at_a_bottom_margin_feeds_history_and_holds_the_rows_below() {
            let mut screen = tall_screen();
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(0),
                bottom: ScreenLine(2),
            });
            for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c'), (3, 'd')] {
                screen.grid[ScreenLine(line)][0].c = glyph;
            }
            screen.state.line = ScreenLine(2);
            let blank = screen.state.pen.erase_cell().c;
            screen.line_feed();
            assert_eq!(screen.grid.history_len(), 1);
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'b');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, 'c');
            assert_eq!(screen.grid[ScreenLine(2)][0].c, blank);
            assert_eq!(screen.grid[ScreenLine(3)][0].c, 'd');
            assert_eq!(screen.grid.row(GridLine(-1))[0].c, 'a');
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
            screen.line_feed();
            assert!(screen.state.pending_wrap);
        }
    }

    mod reverse_index {
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
            let damage = screen.reverse_index();
            assert_eq!(screen.state.line, ScreenLine(1));
            assert_eq!(damage, None);
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
            let damage = screen.reverse_index();
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.grid[ScreenLine(1)][0].c, 'a');
            assert_eq!(damage, Some(DamageSpan::Full));
        }

        /// Asserts that a reverse index disarms the deferred wrap on both
        /// the moving and the scrolling path.
        ///
        /// The agreed policy follows xterm and VTE, whose reverse index
        /// reaches its cursor-up helper on both paths and resets the
        /// flag there. It is a deliberate divergence from kitty and
        /// wezterm, which clear it only when the cursor moves, and
        /// from alacritty, which clears it on neither — and
        /// `Screen::line_feed` preserves the flag, so the split is
        /// not accidental.
        ///
        /// Case: a program fills the last column of a row and then emits a
        /// reverse index instead of the newline the pending wrap was
        /// waiting for.
        #[test]
        fn a_reverse_index_disarms_the_deferred_wrap_on_both_paths() {
            let mut moved = screen();
            moved.state.line = ScreenLine(1);
            moved.state.pending_wrap = true;
            moved.reverse_index();
            assert!(!moved.state.pending_wrap);

            let mut scrolled = screen();
            scrolled.state.pending_wrap = true;
            scrolled.reverse_index();
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
            screen.reverse_index();
            assert_eq!(screen.grid[ScreenLine(0)][0].bg, Color::Indexed(4));
        }

        /// Asserts that a reverse index leaves a scrolled-back viewport
        /// showing what it was showing.
        ///
        /// The agreed policy leaves the display offset alone rather than
        /// adjusting it the way `Screen::line_feed` does. A forward scroll grows
        /// history, so holding the view still requires moving the offset;
        /// a reverse scroll leaves history untouched, so moving the offset
        /// would push the viewport onto different history instead.
        ///
        /// Case: the user has scrolled back to read earlier output while a
        /// full-screen application keeps scrolling its own view backwards.
        #[test]
        fn a_reverse_index_leaves_a_scrolled_viewport_where_it_is() {
            let mut screen = screen();
            for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c')] {
                screen.grid[ScreenLine(line)][0].c = glyph;
            }
            screen.state.line = ScreenLine(2);
            screen.line_feed();
            screen.line_feed();
            screen.set_display_offset(DisplayOffset(1));
            let showing = screen.viewport_row(ViewportLine(0))[0].c;
            screen.state.line = ScreenLine(0);
            screen.reverse_index();
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
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(1),
                bottom: ScreenLine(2),
            });
            screen.grid[ScreenLine(0)][0].c = 'a';
            let damage = screen.reverse_index();
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(damage, None);
        }

        /// Asserts that a cursor above a non-zero top margin still walks
        /// up toward the first row.
        ///
        /// The agreed policy bounds this movement by the screen edge
        /// rather than by the margin: the region gates the scroll alone,
        /// so a cursor outside it moves like an ordinary cursor-up
        /// instead of being pinned at the margin.
        ///
        /// Case: an application sets a scroll region below a two-line
        /// header and emits a reverse index while the cursor sits on the
        /// header's second line.
        #[test]
        fn a_reverse_index_above_a_top_margin_walks_toward_the_first_row() {
            let mut screen = screen();
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(2),
                bottom: ScreenLine(2),
            });
            screen.state.line = ScreenLine(1);
            let damage = screen.reverse_index();
            assert_eq!(screen.state.line, ScreenLine(0));
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
            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(1),
                bottom: ScreenLine(2),
            });
            for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c')] {
                screen.grid[ScreenLine(line)][0].c = glyph;
            }
            screen.state.line = ScreenLine(1);
            let blank = screen.state.pen.erase_cell().c;
            let damage = screen.reverse_index();
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, blank);
            assert_eq!(screen.grid[ScreenLine(2)][0].c, 'b');
            assert_eq!(damage, Some(DamageSpan::Full));
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
            assert_eq!(
                damage,
                Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
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
            screen.line_feed();
            screen.carriage_return();
            for c in ['b', 'c'] {
                screen.print(c);
            }
            screen.state.column = GridColumn(1);
            let damage = screen.erase_in_display(EraseScreenMode::Below);
            assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, 'b');
            assert_eq!(screen.grid[ScreenLine(1)][1].c, ' ');
            assert_eq!(
                damage,
                Some(DamageSpan::rows(ViewportLine(1), ViewportLine(2)))
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
            screen.line_feed();
            screen.carriage_return();
            for c in ['b', 'c', 'd'] {
                screen.print(c);
            }
            screen.state.column = GridColumn(1);
            let damage = screen.erase_in_display(EraseScreenMode::Above);
            assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, ' ');
            assert_eq!(screen.grid[ScreenLine(1)][1].c, ' ');
            assert_eq!(screen.grid[ScreenLine(1)][2].c, 'd');
            assert_eq!(
                damage,
                Some(DamageSpan::rows(ViewportLine(0), ViewportLine(1)))
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
            screen.state.line = ScreenLine(2);
            screen.line_feed();
            screen.carriage_return();
            for c in ['a', 'b'] {
                screen.print(c);
            }
            let damage = screen.erase_in_display(EraseScreenMode::All);
            assert_eq!(screen.grid[ScreenLine(2)][0].c, ' ');
            assert_eq!(screen.grid[ScreenLine(2)][1].c, ' ');
            assert_eq!(screen.grid.history_len(), 1);
            assert_eq!(damage, Some(DamageSpan::Full));
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
            screen.line_feed();
            screen.set_display_offset(DisplayOffset(1));
            screen.state.line = ScreenLine(0);
            assert_eq!(
                screen.erase_in_display(EraseScreenMode::Below),
                Some(DamageSpan::rows(ViewportLine(1), ViewportLine(2)))
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
            screen.line_feed();
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
            screen.line_feed();
            screen.viewport.offset = DisplayOffset(1);
            assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');

            screen.state.line = ScreenLine(2);
            screen.line_feed();
            assert_eq!(screen.display_offset(), DisplayOffset(2));
            assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
        }

        /// Asserts that a scroll inside a region below a non-zero top
        /// margin leaves a scrolled-back viewport's offset untouched.
        ///
        /// The offset counts rows of scrollback, and such a scroll adds
        /// none, so advancing it would slide the view a row further back
        /// than the user put it.
        ///
        /// Case: the user is reading scrollback while a full-screen
        /// application with a pinned header scrolls its pane.
        #[test]
        fn a_scroll_that_adds_no_history_leaves_the_offset_alone() {
            let mut screen = tall_screen();
            for _ in 0..3 {
                screen.state.line = ScreenLine(3);
                screen.line_feed();
            }
            assert_eq!(screen.grid.history_len(), 3);
            screen.viewport.offset = DisplayOffset(1);

            screen.scroll_region.set_margins(Margins {
                top: ScreenLine(1),
                bottom: ScreenLine(3),
            });
            screen.state.line = ScreenLine(3);
            screen.line_feed();
            assert_eq!(screen.display_offset(), DisplayOffset(1));
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
            screen.line_feed();
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
            screen.line_feed();
            screen.viewport.offset = DisplayOffset(1);
            screen.state.line = ScreenLine(2);
            screen.line_feed();
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
            screen.line_feed();
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
            screen.line_feed();
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
                screen.line_feed();
            }
            assert_eq!(screen.viewport_row_of(id), None);
        }
    }

    /// Moves every item `DECSC` saves off its default, so a later
    /// assertion that the state came back cannot pass by accident.
    fn dirty_screen() -> Screen {
        let mut screen = screen();
        screen.state.line = ScreenLine(2);
        screen.state.column = GridColumn(3);
        screen.state.pending_wrap = true;
        screen.pen_mut().bg = Color::Indexed(4);
        screen
            .scroll_region
            .set_origin_mode(OriginMode::WithinMargins);
        screen.invoke_character_set(GCode::G1);
        screen.designate_character_set(GCode::G1, CharacterSet::DecSpecialGraphics);
        screen
    }

    mod save_checkpoint {
        use super::*;

        /// Asserts that a save copies aside every item `DECSC` lists.
        ///
        /// Case: a full-screen application saves its cursor before
        /// drawing a status line in another color and character set.
        #[test]
        fn a_save_copies_every_item_decsc_lists() {
            let mut screen = dirty_screen();
            screen.save_checkpoint();
            assert_eq!(screen.checkpoint.line, ScreenLine(2));
            assert_eq!(screen.checkpoint.column, GridColumn(3));
            assert_eq!(screen.checkpoint.pen.bg, Color::Indexed(4));
            assert!(screen.checkpoint.pending_wrap);
            assert_eq!(screen.checkpoint.origin_mode, OriginMode::WithinMargins);
            assert_eq!(screen.checkpoint.character_set_mapping.gl, GCode::G1);
        }

        /// Asserts that work done after a save leaves the saved copy
        /// alone.
        ///
        /// Case: an application saves its cursor and then keeps printing,
        /// expecting the save to still describe where it was.
        #[test]
        fn later_work_does_not_reach_the_saved_copy() {
            let mut screen = dirty_screen();
            screen.save_checkpoint();
            screen.state.line = ScreenLine(0);
            screen.pen_mut().bg = Color::DefaultBackground;
            screen.invoke_character_set(GCode::G0);
            assert_eq!(screen.checkpoint.line, ScreenLine(2));
            assert_eq!(screen.checkpoint.pen.bg, Color::Indexed(4));
            assert_eq!(screen.checkpoint.character_set_mapping.gl, GCode::G1);
        }
    }

    mod restore_checkpoint {
        use super::*;

        /// Asserts that a restore puts back every item the save copied
        /// aside.
        ///
        /// Case: an application finishes drawing its status line and
        /// returns to where it was working.
        #[test]
        fn a_restore_puts_back_every_saved_item() {
            let mut screen = dirty_screen();
            screen.save_checkpoint();
            screen.state.line = ScreenLine(0);
            screen.state.column = GridColumn(0);
            screen.state.pending_wrap = false;
            screen.pen_mut().bg = Color::DefaultBackground;
            screen
                .scroll_region
                .set_origin_mode(OriginMode::UpperLeftCorner);
            screen.invoke_character_set(GCode::G0);
            screen.restore_checkpoint();
            assert_eq!(screen.state.line, ScreenLine(2));
            assert_eq!(screen.state.column, GridColumn(3));
            assert_eq!(screen.state.pen.bg, Color::Indexed(4));
            assert!(screen.state.pending_wrap);
            assert_eq!(
                screen.scroll_region.origin_mode(),
                OriginMode::WithinMargins
            );
            assert_eq!(screen.character_set_mapping.gl, GCode::G1);
        }

        /// Asserts that a restore with nothing ever saved returns the
        /// screen to its power-up state rather than being ignored.
        ///
        /// Case: an application emits a restore during start-up, before
        /// it has ever saved anything.
        #[test]
        fn an_unsaved_restore_returns_the_power_up_state() {
            let mut screen = dirty_screen();
            screen.restore_checkpoint();
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.state.column, GridColumn(0));
            assert_eq!(screen.state.pen, Pen::default());
            assert!(!screen.state.pending_wrap);
            assert_eq!(
                screen.scroll_region.origin_mode(),
                OriginMode::UpperLeftCorner
            );
            assert_eq!(screen.character_set_mapping, CharacterSetMapping::default());
        }

        /// Asserts that a restored deferred wrap really wraps the next
        /// character.
        ///
        /// The flag is pinned through behaviour rather than by reading it
        /// back, because only the wrap it produces is observable to the
        /// application that saved it.
        ///
        /// Case: an application fills a row to its last column, saves,
        /// goes away to draw elsewhere, restores, and prints one more
        /// character.
        #[test]
        fn a_restored_deferred_wrap_still_wraps_the_next_character() {
            let mut screen = screen();
            for c in ['a', 'b', 'c', 'd'] {
                screen.print(c);
            }
            screen.save_checkpoint();
            screen.state.line = ScreenLine(2);
            screen.state.column = GridColumn(0);
            screen.state.pending_wrap = false;
            screen.restore_checkpoint();
            screen.print('e');
            assert_eq!(screen.grid[ScreenLine(1)][0].c, 'e');
            assert_eq!(screen.state.line, ScreenLine(1));
        }
    }

    mod seat_cursor {
        use super::*;

        /// Asserts that with the origin at the upper-left corner a
        /// relative line is an absolute one.
        ///
        /// Case: a full-screen application addresses the third row of an
        /// unrestricted screen.
        #[test]
        fn an_upper_left_origin_leaves_the_line_absolute() {
            let mut screen = tall_screen();
            screen.seat_cursor(ScreenLine(2), GridColumn(1));
            assert_eq!(screen.state.line, ScreenLine(2));
            assert_eq!(screen.state.column, GridColumn(1));
        }

        /// Asserts that with the origin within the margins a relative
        /// line is measured from the top margin.
        ///
        /// Case: an application pins a header on the first row, turns on
        /// origin mode, and addresses the first row of its own pane.
        #[test]
        fn a_margin_origin_measures_from_the_top_margin() {
            let mut screen = tall_screen();
            screen.scroll_region.set_margins(
                Margins::resolve(Some(2), Some(4), 4).expect("2..=4 is a legal region"),
            );
            screen
                .scroll_region
                .set_origin_mode(OriginMode::WithinMargins);
            screen.seat_cursor(ScreenLine(0), GridColumn(0));
            assert_eq!(screen.state.line, ScreenLine(1));
        }

        /// Asserts that a line past the bottom margin clamps to it while
        /// the origin is within the margins.
        ///
        /// Case: an application with origin mode on addresses a row
        /// below the pane it reserved for itself.
        #[test]
        fn a_line_past_the_bottom_margin_clamps_to_it() {
            let mut screen = tall_screen();
            screen.scroll_region.set_margins(
                Margins::resolve(Some(1), Some(3), 4).expect("1..=3 is a legal region"),
            );
            screen
                .scroll_region
                .set_origin_mode(OriginMode::WithinMargins);
            screen.seat_cursor(ScreenLine(9), GridColumn(0));
            assert_eq!(screen.state.line, ScreenLine(2));
        }

        /// Asserts that a line past the last row clamps to it while the
        /// origin is the upper-left corner.
        ///
        /// Case: an application sized for a taller window addresses row
        /// 40 of a four-row screen.
        #[test]
        fn a_line_past_the_last_row_clamps_to_it() {
            let mut screen = tall_screen();
            screen.seat_cursor(ScreenLine(39), GridColumn(0));
            assert_eq!(screen.state.line, ScreenLine(3));
        }

        /// Asserts that a column past the right edge clamps to the last
        /// column.
        ///
        /// Case: an application sized for a wider window addresses
        /// column 80 of a four-column screen.
        #[test]
        fn a_column_past_the_right_edge_clamps() {
            let mut screen = tall_screen();
            screen.seat_cursor(ScreenLine(0), GridColumn(79));
            assert_eq!(screen.state.column, GridColumn(3));
        }

        /// Asserts that seating the cursor discards a pending deferred
        /// wrap rather than preserving it as a linefeed does.
        ///
        /// Case: an application fills a row to its last column and then
        /// addresses a cell elsewhere instead of printing again.
        #[test]
        fn seating_the_cursor_disarms_the_deferred_wrap() {
            let mut screen = tall_screen();
            screen.state.pending_wrap = true;
            screen.seat_cursor(ScreenLine(0), GridColumn(0));
            assert!(!screen.state.pending_wrap);
        }
    }

    mod set_scroll_region {
        use super::*;

        /// Asserts that a resolved region reaches the scroll span the
        /// line feed and reverse index scroll against.
        ///
        /// Case: an application reserves a status line on the last row
        /// of a four-row screen.
        #[test]
        fn a_resolved_region_reaches_the_scroll_span() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(1), Some(3));
            assert_eq!(
                screen.scroll_region.scroll_span(),
                ScreenLine(0)..=ScreenLine(2)
            );
        }

        /// Asserts that applying a region seats the cursor at the
        /// origin-aware home rather than VT510's "column 1, line 1 of
        /// the page".
        ///
        /// Case: an application sets a region while its cursor sits
        /// somewhere in the middle of the screen.
        #[test]
        fn applying_a_region_seats_the_cursor_at_home() {
            let mut screen = tall_screen();
            screen.state.line = ScreenLine(2);
            screen.state.column = GridColumn(3);
            screen.set_scroll_region(Some(1), Some(3));
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that home follows the origin mode rather than the
        /// page.
        ///
        /// Case: an application turns on origin mode and then moves its
        /// pane down the screen with a second region.
        #[test]
        fn home_follows_the_origin_mode() {
            let mut screen = tall_screen();
            screen
                .scroll_region
                .set_origin_mode(OriginMode::WithinMargins);
            screen.set_scroll_region(Some(2), Some(4));
            assert_eq!(screen.state.line, ScreenLine(1));
        }

        /// Asserts that a refused request leaves both the margins and
        /// the cursor untouched — a whole-sequence no-op rather than a
        /// partial application.
        ///
        /// Case: an application inverts its two parameters and sends
        /// `CSI 5 ; 3 r`.
        #[test]
        fn a_refused_request_changes_nothing() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(1), Some(3));
            screen.state.line = ScreenLine(2);
            screen.set_scroll_region(Some(5), Some(3));
            assert_eq!(
                screen.scroll_region.scroll_span(),
                ScreenLine(0)..=ScreenLine(2)
            );
            assert_eq!(screen.state.line, ScreenLine(2));
        }
    }

    mod set_origin_mode {
        use super::*;

        /// Asserts that setting the origin within the margins seats the
        /// cursor at the top margin.
        ///
        /// Case: an application reserves a header row, then turns on
        /// origin mode so its own coordinates start below it.
        #[test]
        fn setting_the_origin_seats_the_cursor_at_the_top_margin() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(2), Some(4));
            screen.state.line = ScreenLine(3);
            screen.set_origin_mode(OriginMode::WithinMargins);
            assert_eq!(screen.state.line, ScreenLine(1));
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that resetting the origin homes the cursor at the
        /// upper-left corner rather than homing on set alone.
        ///
        /// Case: a full-screen application drops origin mode on its way
        /// out and prints without addressing the cursor first.
        #[test]
        fn resetting_the_origin_seats_the_cursor_at_the_corner() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(2), Some(4));
            screen.set_origin_mode(OriginMode::WithinMargins);
            screen.state.line = ScreenLine(3);
            screen.set_origin_mode(OriginMode::UpperLeftCorner);
            assert_eq!(screen.state.line, ScreenLine(0));
        }

        /// Asserts that the mode reaches the region the cursor motion
        /// reads.
        ///
        /// Case: an application turns on origin mode and the terminal
        /// has to answer later cursor addressing against the margins.
        #[test]
        fn the_mode_reaches_the_scroll_region() {
            let mut screen = tall_screen();
            screen.set_origin_mode(OriginMode::WithinMargins);
            assert_eq!(
                screen.scroll_region.origin_mode(),
                OriginMode::WithinMargins
            );
        }
    }

    mod move_cursor_to {
        use super::*;

        /// Asserts that omitted parameters address the first line and
        /// column.
        ///
        /// Case: an application homes the cursor with a bare `CSI H`.
        #[test]
        fn omitted_parameters_address_the_first_cell() {
            let mut screen = tall_screen();
            screen.state.line = ScreenLine(2);
            screen.state.column = GridColumn(3);
            screen.move_cursor_to(None, None);
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that a zero addresses the first line and column, the
        /// same as a one.
        ///
        /// The agreed policy follows VT510 p.116 — "If Pl or Pc is not
        /// selected or selected as 0, then the cursor moves to the first
        /// line or column".
        ///
        /// Case: a program that builds its sequences from zero-based
        /// variables emits `CSI 0 ; 0 H`.
        #[test]
        fn a_zero_addresses_the_first_cell() {
            let mut screen = tall_screen();
            screen.state.line = ScreenLine(2);
            screen.move_cursor_to(Some(0), Some(0));
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that one-based parameters land on zero-based cells.
        ///
        /// Case: a full-screen application draws a box corner by
        /// addressing row 3, column 2.
        #[test]
        fn one_based_parameters_land_on_zero_based_cells() {
            let mut screen = tall_screen();
            screen.move_cursor_to(Some(3), Some(2));
            assert_eq!(screen.state.line, ScreenLine(2));
            assert_eq!(screen.state.column, GridColumn(1));
        }

        /// Asserts that the line is measured from the top margin while
        /// the origin is within the margins.
        ///
        /// Case: an application with a reserved header turns on origin
        /// mode and addresses the first row of its own pane.
        #[test]
        fn a_margin_origin_measures_the_line_from_the_top_margin() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(2), Some(4));
            screen.set_origin_mode(OriginMode::WithinMargins);
            screen.state.line = ScreenLine(3);
            screen.state.column = GridColumn(3);
            screen.move_cursor_to(Some(1), Some(1));
            assert_eq!(screen.state.line, ScreenLine(1));
        }

        /// Asserts that the line is absolute and reaches outside the
        /// margins while the origin is the upper-left corner.
        ///
        /// The agreed policy follows VT510 p.195: with `DECOM` reset the
        /// line numbering is independent of the margins and the cursor
        /// can move outside them.
        ///
        /// Case: an application keeps a scrolling pane but addresses the
        /// header row above it to update a title.
        #[test]
        fn an_upper_left_origin_reaches_outside_the_margins() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(2), Some(4));
            screen.state.line = ScreenLine(3);
            screen.state.column = GridColumn(3);
            screen.move_cursor_to(Some(1), Some(1));
            assert_eq!(screen.state.line, ScreenLine(0));
        }

        /// Asserts that a line past the region clamps to the bottom
        /// margin while the origin is within the margins.
        ///
        /// Case: an application with origin mode on addresses a row
        /// below the pane it reserved for itself.
        #[test]
        fn a_line_past_the_region_clamps_to_the_bottom_margin() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(1), Some(3));
            screen.set_origin_mode(OriginMode::WithinMargins);
            screen.move_cursor_to(Some(9), Some(1));
            assert_eq!(screen.state.line, ScreenLine(2));
        }
    }

    mod reset {
        use super::*;

        /// Asserts that a reset blanks every visible cell and reports
        /// the whole screen as damaged.
        ///
        /// Case: a full-screen application exits and the shell sends
        /// `RIS` to take the terminal back to a known state.
        #[test]
        fn a_reset_empties_every_visible_cell() {
            let mut screen = screen();
            for line in 0..3 {
                for column in 0..4 {
                    screen.grid[ScreenLine(line)][column].c = 'x';
                }
            }
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            for line in 0..3 {
                let row = screen.viewport_row(ViewportLine(line));
                assert!(row.iter().all(|cell| *cell == Cell::default()));
            }
        }

        /// Asserts that a reset fills the grid with default cells rather
        /// than carrying the pen background into them the way an erase
        /// does.
        ///
        /// Case: an application paints a red-backgrounded banner and the
        /// shell resets the terminal without the application restoring
        /// SGR first.
        #[test]
        fn a_reset_leaves_no_trace_of_the_pen_background_in_the_cells() {
            let mut screen = screen();
            screen.pen_mut().bg = Color::Indexed(1);
            screen.print('x');
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(screen.viewport_row(ViewportLine(0))[0], Cell::default());
        }

        /// Asserts that a reset seats the cursor at the upper-left
        /// corner of the screen.
        ///
        /// Case: a shell sends `RIS` while its cursor sits mid-screen
        /// after a half-drawn prompt.
        #[test]
        fn a_reset_homes_the_cursor() {
            let mut screen = screen();
            screen.grid[ScreenLine(0)][0].c = 'x';
            screen.move_cursor_to(Some(2), Some(3));
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that a reset discards the scrollback history and
        /// reseats the viewport on the live tail.
        ///
        /// Case: the user has an earlier command's output scrolled back
        /// into view when the next one sends `RIS`.
        #[test]
        fn a_reset_drops_the_scrollback_history() {
            let mut screen = screen();
            screen.grid[ScreenLine(0)][0].c = 'a';
            screen.state.line = ScreenLine(2);
            screen.line_feed();
            screen.grid[ScreenLine(0)][0].c = 'b';
            screen.viewport.offset = DisplayOffset(1);
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(screen.grid.history_len(), 0);
            assert_eq!(screen.display_offset(), DisplayOffset(0));
        }

        /// Asserts that a reset returns the SGR pen to normal rendition.
        ///
        /// Case: the shell resets a terminal an application left with a
        /// red background selected, then prints its own prompt.
        #[test]
        fn a_reset_returns_the_pen_to_normal_rendition() {
            let mut screen = screen();
            screen.pen_mut().bg = Color::Indexed(1);
            screen.print('x');
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(*screen.pen_mut(), Pen::default());
        }

        /// Asserts that a reset blanks the rows outside the scrolling
        /// region as well as the ones inside it.
        ///
        /// Case: an application reserves a status line outside its
        /// scrolling region and is then reset.
        #[test]
        fn a_reset_empties_rows_outside_the_scrolling_region_as_well() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(2), Some(3));
            for line in 0..4 {
                screen.grid[ScreenLine(line)][0].c = 'x';
            }
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(screen.viewport_row(ViewportLine(0))[0], Cell::default());
            assert_eq!(screen.viewport_row(ViewportLine(3))[0], Cell::default());
        }

        /// Asserts that a reset restores the full-page margins and the
        /// absolute origin, and homes to the screen's corner rather than
        /// to the margin the old origin mode defined.
        ///
        /// Case: a full-screen editor with a reserved status line and
        /// origin mode on is reset by the shell that outlives it.
        #[test]
        fn a_reset_restores_the_margins_and_homes_to_the_screen_corner() {
            let mut screen = tall_screen();
            screen.set_scroll_region(Some(2), Some(3));
            screen.set_origin_mode(OriginMode::WithinMargins);
            screen.grid[ScreenLine(0)][0].c = 'x';
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(screen.scroll_region.top_margin(), ScreenLine(0));
            assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(3));
            assert_eq!(
                screen.scroll_region.origin_mode(),
                OriginMode::UpperLeftCorner
            );
            assert_eq!(screen.state.line, ScreenLine(0));
        }

        /// Asserts that a reset returns every G code and the GL
        /// invocation to their defaults.
        ///
        /// Case: a curses application locks the DEC line-drawing set
        /// into GL and dies without restoring ASCII.
        #[test]
        fn a_reset_returns_the_character_sets_to_their_defaults() {
            let mut screen = screen();
            screen.designate_character_set(GCode::G1, CharacterSet::DecSpecialGraphics);
            screen.invoke_character_set(GCode::G1);
            screen.grid[ScreenLine(0)][0].c = 'x';
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            assert_eq!(screen.character_set_mapping, CharacterSetMapping::default());
        }

        /// Asserts that a reset reinstalls the default eight-column
        /// tabulation stride.
        ///
        /// Case: an application clears every stop with `TBC 3`, installs
        /// one of its own, and is reset before it restores the defaults.
        #[test]
        fn a_reset_reinstalls_the_default_tabulation_stride() {
            let mut screen = wide_screen();
            screen.edit_tab_stop(CharacterTabEdit::ClearAllColumns);
            screen.state.column = GridColumn(3);
            screen.set_horizontal_tab_stop();
            screen.grid[ScreenLine(0)][0].c = 'x';
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
            screen.move_forward_tabs(1);
            assert_eq!(screen.state.column, GridColumn(8));
        }

        /// Asserts that a reset of an already-blank screen with no
        /// history reports no damage, and homes the cursor anyway.
        ///
        /// Case: the user sends `RIS` twice in a row at a fresh prompt.
        #[test]
        fn a_reset_of_an_already_blank_screen_reports_no_damage() {
            let mut screen = screen();
            screen.move_cursor_to(Some(2), Some(3));
            assert_eq!(screen.reset(), None);
            assert_eq!(screen.state.line, ScreenLine(0));
            assert_eq!(screen.state.column, GridColumn(0));
        }

        /// Asserts that one dirty cell is enough to report the whole
        /// screen as damaged.
        ///
        /// Case: a background job prints a single character into the
        /// corner of an otherwise untouched screen before the reset.
        #[test]
        fn a_single_dirty_cell_still_reports_the_whole_screen() {
            let mut screen = screen();
            screen.grid[ScreenLine(2)][3].c = 'x';
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
        }

        /// Asserts that history alone makes a reset report damage, with
        /// every visible cell already blank.
        ///
        /// Case: a long build scrolls its output away and leaves a blank
        /// screen, and the user resets to reclaim the scrollback.
        #[test]
        fn a_blank_screen_with_history_still_reports_the_whole_screen() {
            let mut screen = screen();
            screen.state.line = ScreenLine(2);
            screen.line_feed();
            assert_eq!(screen.grid.history_len(), 1);
            assert_eq!(screen.reset(), Some(DamageSpan::Full));
        }
    }
}
