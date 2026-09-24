//! The atomic grid + cursor operation unit for one terminal screen.

pub mod cell;
pub mod character_sets;
pub mod checkpoint;
pub mod grid;
pub mod margins;
pub mod selection;
pub mod tabs;
pub mod viewport;

pub(crate) mod cursor;
pub(crate) mod webview_placements;

mod state;

use self::cell::{BodyWidth, Cell, CellExtra, CellWidth, ClassifiedGlyph, Pen};
use self::grid::Grid;
use self::grid::LineId;
use self::grid::row::Row;
use crate::device::modes::{
    AutoWrap, CursorBlink, InsertReplaceMode, TextCursorEnable, TextCursorModes,
};
use crate::error::VtResult;
use crate::frame::damage::DamageSpan;
use crate::hyperlink::HyperlinkId;
use crate::placement::{AnchoredPlacement, InstanceId, PlacementSize};
use crate::screen::character_sets::{
    CharacterSet, CharacterSetMapping, GCode, GraphicChar, SingleShift,
};
use crate::screen::checkpoint::Checkpoint;
use crate::screen::cursor::Cursor;
use crate::screen::grid::GridSize;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
use crate::screen::grid::reflow::{ScrollbackOnGrow, TrackedPoint};
use crate::screen::margins::{Margins, OriginMode, ScrollRegion};
use crate::screen::selection::{
    CellSide, Resolved, ScreenSelection, SelectionEnd, SelectionGeometry, SelectionKind,
    SelectionRange,
};
use crate::screen::state::ScreenState;
use crate::screen::tabs::{CharacterTabEdit, TabStops};
use crate::screen::viewport::{DisplayOffset, Scroll, Viewport, ViewportLine};
use crate::screen::webview_placements::WebviewPlacements;
use std::ops::{Range, RangeInclusive};

/// One terminal screen: cell storage plus the write cursor, updated
/// atomically by each operation.
///
/// A mutation that damages rows returns the [`DamageSpan`] it produced
/// for the caller to stage; pure cursor motion returns nothing.
///
/// The caller must reject a size with a zero axis before it reaches
/// [`Self::new`], [`Self::resize`] or [`Self::reflow`].
///
/// # Invariants
///
/// Both grid axes are nonzero.
#[derive(Debug)]
pub struct Screen {
    grid: Grid,
    viewport: Viewport,
    state: ScreenState,
    scroll_region: ScrollRegion,
    tabs: TabStops,
    character_set_mapping: CharacterSetMapping,
    checkpoint: Checkpoint,
    webview_placements: WebviewPlacements,
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
    /// `ED 3`, which erases the scrollback, is not answered: every span
    /// here is confined to the visible screen.
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
            webview_placements: WebviewPlacements::new(),
            selection: ScreenSelection::new(),
        }
    }
}

/// The device state a printed character is shaped by.
///
/// The default value prints in replace mode, with autowrap set, outside
/// any hyperlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrintOptions {
    /// `IRM`: under [`InsertReplaceMode::Insert`] the rest of the row shifts
    /// right one column before the character lands.
    pub insert_replace: InsertReplaceMode,
    /// `DECAWM`: while it is reset, a character at the right border replaces
    /// the last column, and an armed wrap, such as one a `DECRC` restored, is
    /// not resolved either.
    pub auto_wrap: AutoWrap,
    /// The hyperlink the character is printed inside, or `None` when no link
    /// is open.
    pub hyperlink_id: Option<HyperlinkId>,
}

/// Graphic character output.
impl Screen {
    /// Maps `c` through the character set a pending single shift invokes,
    /// consuming that single shift, or otherwise through the set invoked
    /// into GL.
    pub fn translate(&mut self, c: char) -> GraphicChar {
        self.character_set_mapping.translate(c)
    }

    /// Disarms the deferred wrap, leaving the cursor and the cells
    /// alone.
    ///
    /// The saved cursor keeps its own flag: DEC STD-070 has `DECSC` carry
    /// the last-column flag.
    ///
    /// # Control Functions
    ///
    /// - `DECRST 7` (`CSI ? 7 l`) — the pending-wrap part
    pub fn disarm_pending_wrap(&mut self) {
        self.state.pending_wrap = false;
    }

    /// Prints a glyph already mapped through a character set at the cursor
    /// with the current pen, as `options` shape it, wrapping first when the
    /// deferred wrap is armed and autowrap is set.
    ///
    /// A pending single shift stays pending.
    ///
    /// A one-column glyph takes the cursor's cell and a two-column glyph
    /// takes it and the next; a zero-width mark joins the glyph the
    /// cursor last passed and leaves the cursor alone.
    ///
    /// A two-column glyph with one column left wraps first, leaving a
    /// filler in the last column; with autowrap reset it is dropped and
    /// the deferred wrap is disarmed. A two-column glyph on a one-column
    /// screen is dropped.
    ///
    /// Under [`InsertReplaceMode::Insert`] the rest of the row shifts
    /// right by the glyph's width before it lands.
    ///
    /// Reports [`DamageSpan::Full`] when a wrap scrolled, otherwise every
    /// row the print touched, or `None` when nothing changed or the rows
    /// have scrolled out of the window.
    ///
    /// # Errors
    ///
    /// [`VtError::Stamp`](crate::error::VtError::Stamp) when the row
    /// refuses the glyph or the filler a wrapping two-column glyph leaves;
    /// the cursor and the deferred wrap are then left as they stood when
    /// the row refused.
    pub(crate) fn print(
        &mut self,
        classified: ClassifiedGlyph,
        options: PrintOptions,
    ) -> VtResult<Option<DamageSpan>> {
        let glyph = classified.glyph();
        let class = classified.class();
        let Some(width) = class.body_width() else {
            return Ok(self.attach_zero_width(glyph));
        };
        let Some(damage) = self.make_room_for_glyph(width, options.auto_wrap)? else {
            return Ok(None);
        };
        self.land_glyph(glyph, width, options)?;
        Ok(damage.span(self))
    }

    /// Combines `mark` onto the glyph the cursor last passed: the cell
    /// under the cursor when the deferred wrap is armed, or when the
    /// cursor is on the last column and the last printed glyph landed
    /// there, otherwise the cell to its left, and on column zero that
    /// column's own cell. A continuation column hands the mark to its
    /// wide body.
    ///
    /// Reports the cursor's row when the mark was kept, and `None` when
    /// the cell already holds [`cell::MAX_COMBINING`] marks or the target is a
    /// filler.
    fn attach_zero_width(&mut self, mark: char) -> Option<DamageSpan> {
        let column = self.state.column.0;
        let cursor = (self.state.line, self.state.column);
        let on_landing = self.state.last_landing == Some(cursor);
        let candidate = if self.state.pending_wrap || (self.is_last_column() && on_landing) {
            column
        } else {
            column.saturating_sub(1)
        };
        let line = self.state.line;
        let row = &mut self.grid[line];
        let target = match row[candidate].width {
            CellWidth::Narrow | CellWidth::Wide => candidate,
            CellWidth::Spacer => candidate.saturating_sub(1),
            CellWidth::LeadingSpacer => return None,
        };
        let extra = row[target]
            .extra
            .get_or_insert_with(|| Box::new(CellExtra::default()));
        if !extra.push(mark) {
            return None;
        }
        self.damage_span(line, line)
    }

    /// Moves the cursor to where a glyph `width` wide lands: under autowrap
    /// an armed deferred wrap resolves first, and a glyph that overflows
    /// the row wraps, leaving a filler in the last column.
    ///
    /// Returns the damage the print reports once the glyph lands, or
    /// `None` when the glyph is dropped: it is wider than the screen, or it
    /// overflows the row with autowrap reset, which also disarms the
    /// deferred wrap.
    ///
    /// A wrap records, on the row it leaves, how many of that row's cells
    /// continue on the next one; a filler in the row's last column is not
    /// among them.
    ///
    /// # Errors
    ///
    /// [`VtError::Stamp`](crate::error::VtError::Stamp) when the row
    /// refuses the filler; the cursor and the deferred wrap are then left
    /// untouched.
    fn make_room_for_glyph(
        &mut self,
        width: BodyWidth,
        auto_wrap: AutoWrap,
    ) -> VtResult<Option<PrintDamage>> {
        let columns = width.columns();
        let cols = self.grid.size().cols;
        if cols < columns {
            return Ok(None);
        }
        let wrapping = auto_wrap.wraps();
        let mut scrolled = false;
        if self.state.pending_wrap && wrapping {
            scrolled = self.wrap_recording();
        }
        let first_line = self.state.line;
        if !self.fits(columns) {
            if !wrapping {
                self.state.pending_wrap = false;
                return Ok(None);
            }
            let pen = self.state.pen;
            self.grid[self.state.line].place_filler(&pen)?;
            scrolled |= self.wrap_recording();
        }
        Ok(Some(PrintDamage {
            first_line,
            last_line: self.state.line,
            scrolled,
        }))
    }

    /// Rewinds the cursor to column zero and moves it down one row,
    /// scrolling at the bottom margin; the deferred wrap is left as it is.
    ///
    /// Returns whether the move scrolled.
    fn wrap_to_next_line(&mut self) -> bool {
        self.state.column = GridColumn(0);
        self.line_feed().is_some()
    }

    /// Wraps the cursor onto the next row and records that the row it left
    /// continues there with every cell but a filler in its last column;
    /// returns whether the move scrolled.
    ///
    /// Nothing is recorded when the row left behind does not end up
    /// directly above the cursor: the move stayed on its row, or a region
    /// scroll discarded the row.
    fn wrap_recording(&mut self) -> bool {
        let cols = self.grid.size().cols;
        let ends_in_filler = self.grid[self.state.line]
            .last()
            .is_some_and(|cell| cell.width == CellWidth::LeadingSpacer);
        let cells = if ends_in_filler {
            cols.saturating_sub(1)
        } else {
            cols
        };
        let departed = self.cursor_line_id();
        let scrolled = self.wrap_to_next_line();
        let above = GridLine(i32::from(self.state.line.0) - 1);
        if self.grid.line_id_at(above) == Some(departed) {
            self.grid.set_wrap_at(above, cells);
        }
        scrolled
    }

    /// Whether a glyph spanning `width` columns fits from the cursor's
    /// column through the row's end.
    fn fits(&self, width: u16) -> bool {
        width <= self.grid.size().cols.saturating_sub(self.state.column.0)
    }

    /// Stamps `glyph` at the cursor with the current pen and advances the
    /// cursor past it; under [`InsertReplaceMode::Insert`] the rest of the
    /// row first shifts right by the glyph's width.
    ///
    /// The glyph must fit between the cursor and the row's end, and under
    /// autowrap an armed deferred wrap must already have been resolved.
    ///
    /// A glyph reaching the last column ends the row's logical line; a wrap
    /// that resolves later records it again. A glyph landing past a
    /// recorded wrap that stops short of the last column extends the wrap
    /// to cover it.
    ///
    /// # Errors
    ///
    /// [`VtError::Stamp`](crate::error::VtError::Stamp) when the row
    /// refuses the glyph; the cursor then does not advance.
    fn land_glyph(&mut self, glyph: char, width: BodyWidth, options: PrintOptions) -> VtResult {
        // NOTE: The shift runs after `make_room_for_glyph` has resolved the
        // deferred wrap. Shifting first would let `insert_characters` clear
        // `pending_wrap`, and the character would overwrite the last column
        // instead of wrapping to the next row.
        if matches!(options.insert_replace, InsertReplaceMode::Insert) {
            self.insert_characters(width.columns());
        }
        let (line, column) = (self.state.line, self.state.column);
        self.grid.stamp_visible(
            line,
            column,
            glyph,
            width,
            &self.state.pen,
            options.hyperlink_id,
        )?;
        self.state.last_landing = Some((line, column));
        self.advance_past_glyph(width, options.auto_wrap);
        Ok(())
    }

    /// Advances the cursor past a glyph `width` wide that landed at the
    /// cursor.
    ///
    /// A glyph that ends the row parks the cursor on the last column and
    /// arms the deferred wrap only under autowrap; any other glyph moves
    /// the cursor right and disarms it.
    fn advance_past_glyph(&mut self, width: BodyWidth, auto_wrap: AutoWrap) {
        let cols = self.grid.size().cols;
        let next = self.state.column.0.saturating_add(width.columns());
        if next >= cols {
            self.state.column = GridColumn(cols - 1);
            self.state.pending_wrap = auto_wrap.wraps();
        } else {
            self.state.column = GridColumn(next);
            self.state.pending_wrap = false;
        }
    }
}

/// Cursor addressing.
///
/// None of these report damage.
impl Screen {
    /// Moves the cursor one column left and disarms the deferred wrap.
    ///
    /// A backspace at column zero stays there.
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
    /// The bottom margin is the barrier: a cursor at or above it stops
    /// there, and only a cursor already below it reaches the last row.
    ///
    /// # Control Functions
    ///
    /// - `CUD` (`CSI Pn B`)
    /// - `CNL` (`CSI Pn E`) — before its carriage return
    pub fn move_cursor_down(&mut self, count: u16) {
        let bottom = self.scroll_region.bottom_margin();
        self.move_cursor_down_stopping_at(bottom, count);
    }

    /// Moves the cursor down `count` rows in the same column, never
    /// scrolling.
    ///
    /// The last row of the addressable page is the barrier: the bottom
    /// margin is passed rather than stopping the cursor, and only origin
    /// mode stops it at the bottom margin. A cursor that already sits
    /// below that barrier still moves down, as far as the last row of
    /// the page. A pending deferred wrap is discarded.
    ///
    /// # Control Functions
    ///
    /// - `VPR` (`CSI Pn e`)
    ///
    /// # References
    ///
    /// - vt510.pdf p.351 — "If an attempt is made to move the active
    ///   position below the last line, the active position stops at the
    ///   last line."
    pub fn move_cursor_down_within_page(&mut self, count: u16) {
        let last = self.last_addressable_line();
        self.move_cursor_down_stopping_at(last, count);
    }

    /// Moves the cursor `count` columns left, stopping at the first
    /// column.
    ///
    /// The page border is the barrier, not a margin: this terminal has
    /// no left margin.
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
    /// The page border is the barrier.
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
    /// The line is resolved against the current [`OriginMode`] and both
    /// axes are clamped, so a line outside the addressable region stops
    /// at its edge rather than being refused.
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
    /// past the last stops there. The row is never touched, whatever the
    /// origin mode, and a pending deferred wrap is discarded.
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
    /// refused. The column is never touched, but a pending deferred wrap
    /// is still discarded.
    ///
    /// # Control Functions
    ///
    /// - `VPA` (`CSI Pn d`)
    pub fn move_cursor_to_line(&mut self, line: Option<u16>) {
        self.seat_line(ScreenLine(Self::addressed_index(line)));
    }

    /// Seats the cursor at `line` — measured from the origin the current
    /// [`OriginMode`] defines — and `column`, clamping both axes and
    /// disarming the deferred wrap.
    fn seat_cursor(&mut self, line: ScreenLine, column: GridColumn) {
        self.seat_line(line);
        self.seat_column(column);
    }

    /// Seats the cursor at `line`, measured from the origin the current
    /// [`OriginMode`] defines, clamping it to the addressable region and
    /// disarming the deferred wrap, without touching the column.
    fn seat_line(&mut self, line: ScreenLine) {
        let origin = self.addressable_origin();
        let last = self.last_addressable_line();
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

    /// Moves the cursor down `count` rows, stopping at `barrier`, or at
    /// the last row of the page when the cursor already sits below
    /// `barrier`.
    fn move_cursor_down_stopping_at(&mut self, barrier: ScreenLine, count: u16) {
        let limit = if self.state.line <= barrier {
            barrier
        } else {
            self.last_page_line()
        };
        self.state.line = ScreenLine(self.state.line.0.saturating_add(count).min(limit.0));
        self.state.pending_wrap = false;
    }

    /// The first line the current [`OriginMode`] addresses, which a
    /// one-based line parameter is measured from.
    fn addressable_origin(&self) -> ScreenLine {
        match self.scroll_region.origin_mode() {
            OriginMode::WithinMargins => self.scroll_region.top_margin(),
            OriginMode::UpperLeftCorner => ScreenLine(0),
        }
    }

    /// The last line the current [`OriginMode`] addresses, the ceiling an
    /// addressed line is clamped to.
    fn last_addressable_line(&self) -> ScreenLine {
        match self.scroll_region.origin_mode() {
            OriginMode::WithinMargins => self.scroll_region.bottom_margin(),
            OriginMode::UpperLeftCorner => self.last_page_line(),
        }
    }

    /// The last line of the page, whatever the current [`OriginMode`]
    /// addresses.
    fn last_page_line(&self) -> ScreenLine {
        ScreenLine(self.grid.size().rows - 1)
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
    /// the deferred-wrap flag is preserved.
    ///
    /// A move inside the screen reports nothing, and a scroll reports
    /// [`DamageSpan::Full`].
    ///
    /// A cursor below a non-zero bottom margin and already on the last
    /// row moves nothing and scrolls nothing.
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
    /// the bottom margin. An insert never feeds history, and the cursor
    /// homing follows ECMA-48 § 8.3.67.
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
    /// the page feeds the deleted rows to history, and the cursor homing
    /// follows ECMA-48 § 8.3.32.
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
    /// disarms the deferred wrap; a zero count touches nothing, that
    /// flag included.
    ///
    /// The edit applies wherever the cursor sits, ignoring the scrolling
    /// margins VT510 gates `ICH` on. Selection and placement anchors hold
    /// absolute columns and do not move with the content.
    ///
    /// A recorded wrap that stops short of the last column moves with the
    /// text the shift moves.
    ///
    /// # Control Functions
    ///
    /// - `ICH` (`CSI Pn @`)
    /// - `IRM` (`CSI 4 h`) — the shift [`Self::print`] performs
    ///   for each character printed in insert mode
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
    /// last one. A shift disarms the deferred wrap; a zero count touches
    /// nothing, that flag included.
    ///
    /// The edit applies wherever the cursor sits, ignoring the scrolling
    /// margins VT510 gates `DCH` on. Selection and placement anchors hold
    /// absolute columns and do not move with the content.
    ///
    /// A recorded wrap that stops short of the last column moves with the
    /// text the shift moves.
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
    /// - `SD` (`CSI Pn ^`), xterm's alternate spelling
    pub fn scroll_region_down(&mut self, count: u16) -> Option<DamageSpan> {
        self.shift_rows_down(self.scroll_region.top_margin(), count)
    }

    /// Follows a one-row scroll with the offset that keeps a scrolled
    /// viewport on the content it was showing.
    ///
    /// A viewport pinned to the live tail stays pinned, and a scrolled one
    /// counts one row further back. At capacity the row the user was
    /// reading has been evicted, so the view drifts by one.
    ///
    /// # Invariants
    ///
    /// The offset is clamped to the history that survives the scroll.
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
    /// the row it was showing. It also clears the cursor's landing
    /// cell, since the rows moved under it.
    fn shift_rows_up(&mut self, first: ScreenLine, count: u16) -> Option<DamageSpan> {
        let bottom = self.scroll_region.bottom_margin();
        let count = self.clamped_rows(first, count)?;
        self.state.last_landing = None;
        let fill = self.state.pen.erase_cell();
        let feeds_history = first == ScreenLine(0);
        for _ in 0..count {
            self.grid.scroll_up_one(first, bottom, fill.clone());
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
    /// The rows pushed past the bottom margin are discarded, and nothing
    /// reaches history. It also clears the cursor's landing cell, since
    /// the rows moved under it.
    fn shift_rows_down(&mut self, first: ScreenLine, count: u16) -> Option<DamageSpan> {
        let bottom = self.scroll_region.bottom_margin();
        let count = self.clamped_rows(first, count)?;
        self.state.last_landing = None;
        let fill = self.state.pen.erase_cell();
        for _ in 0..count {
            self.grid.scroll_down_one(first, bottom, fill.clone());
        }
        Some(DamageSpan::Full)
    }

    /// The rows a shift starting at `first` may actually move: `count`
    /// clamped to the rows through the bottom margin, and `None` when
    /// that leaves nothing to do.
    ///
    /// A `first` below the bottom margin also yields `None`.
    fn clamped_rows(&self, first: ScreenLine, count: u16) -> Option<u16> {
        let bottom = self.scroll_region.bottom_margin();
        let count = count.min(bottom.0.checked_sub(first.0)? + 1);
        (count > 0).then_some(count)
    }

    /// The columns an in-row edit at the cursor may actually touch:
    /// `count` clamped to the columns from the cursor through the last
    /// one, and `None` when that leaves nothing to do.
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
    /// Erases part of the cursor row with the pen colors (BCE).
    ///
    /// [`EraseLineMode::ToEnd`] is a no-op while the cursor logically sits
    /// past the row, with the deferred wrap armed on the last column and
    /// `DECAWM` set.
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
    /// pen colors (BCE), leaving the cursor where it is.
    ///
    /// It is a no-op while the cursor logically sits past the row, with
    /// the deferred wrap armed on the last column and `DECAWM` set.
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

    /// Erases part of the visible screen with the pen colors (BCE), in
    /// place; scrollback history is never touched.
    ///
    /// # Control Functions
    ///
    /// - `ED` (`CSI Ps J`)
    pub fn erase_in_display(&mut self, mode: EraseScreenMode) -> Option<DamageSpan> {
        let GridSize { cols, rows } = self.grid.size();
        let blank = self.state.pen.erase_cell();
        match mode {
            EraseScreenMode::Below => {
                self.grid.fill_visible_row_range(
                    self.state.line,
                    self.state.column.0..cols,
                    blank.clone(),
                );
                for line in self.state.line.0 + 1..rows {
                    self.grid
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank.clone());
                }
                self.damage_span(self.state.line, ScreenLine(rows - 1))
            }
            EraseScreenMode::Above => {
                for line in 0..self.state.line.0 {
                    self.grid
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank.clone());
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
                        .fill_visible_row_range(ScreenLine(line), 0..cols, blank.clone());
                }
                Some(DamageSpan::Full)
            }
        }
    }

    /// Fills the given column range of the cursor row with the pen's
    /// erase cell and reports that row.
    fn erase_cursor_row_columns(&mut self, columns: Range<u16>) -> Option<DamageSpan> {
        self.grid
            .fill_visible_row_range(self.state.line, columns, self.state.pen.erase_cell());
        self.damage_span(self.state.line, self.state.line)
    }

    /// Whether the cursor logically sits past the row's last cell.
    ///
    /// All three conditions are required: the deferred wrap armed,
    /// `DECAWM` set, and the cursor on the last column.
    fn cursor_parked_past_the_row(&self, auto_wrap: AutoWrap) -> bool {
        self.state.pending_wrap && auto_wrap.wraps() && self.is_last_column()
    }

    /// Whether the cursor is on the row's last column.
    fn is_last_column(&self) -> bool {
        self.state.column.0 + 1 >= self.grid.size().cols
    }
}

/// Tabulation stops.
impl Screen {
    /// Moves the cursor forward `count` tabulation stops.
    ///
    /// The right edge is this screen's last column.
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
    /// The left edge is column zero.
    ///
    /// TODO: take the left edge from the left margin once `DECSLRM` is
    /// implemented.
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
    /// TODO: scope HTS by TABULATION STOP MODE once that mode is
    /// implemented.
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
    /// The deferred wrap is left as it is.
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
    /// for an omitted one. An omitted or zero top means the first line
    /// and an omitted or zero bottom the last, and a bottom past the page
    /// is clamped to the last line. A request whose top is not above its
    /// bottom is refused.
    ///
    /// The cursor goes to the home the origin mode defines rather than
    /// to the "column 1, line 1 of the page" VT510 p.276 states.
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

    /// Whether the cursor origin follows the margins (`DECOM`).
    pub fn origin_mode(&self) -> OriginMode {
        self.scroll_region.origin_mode()
    }

    /// Sets the cursor origin and seats the cursor at the home the new
    /// mode defines.
    ///
    /// Both directions seat the cursor.
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
    /// history. A scrolled read must come through here.
    pub fn viewport_row(&self, line: ViewportLine) -> &Row<Cell> {
        self.grid.row(line.to_grid(self.viewport.offset))
    }

    /// Number of scrollback rows the viewport sits above the live tail.
    #[inline]
    pub const fn display_offset(&self) -> DisplayOffset {
        self.viewport.offset
    }

    /// Moves the viewport by one [`Scroll`] motion; `None` when the
    /// motion was zero or entirely clamped away.
    ///
    /// A screen that keeps no history never moves, so this is a no-op on
    /// the alternate screen.
    ///
    /// # Invariants
    ///
    /// A motion that moves the viewport reports [`DamageSpan::Full`].
    pub fn scroll(&mut self, scroll: Scroll) -> Option<DamageSpan> {
        let before = self.viewport.offset;
        self.set_display_offset(self.scroll_target(scroll));
        (self.viewport.offset != before).then_some(DamageSpan::Full)
    }

    /// Seats the viewport at `offset`, clamped to the history that
    /// currently exists.
    ///
    /// The caller must stage full damage.
    pub fn set_display_offset(&mut self, offset: DisplayOffset) {
        let history =
            u32::try_from(self.grid.history_len()).expect("scrollback never exceeds u32::MAX rows");
        self.viewport.offset = DisplayOffset(offset.0.min(history));
    }
}

/// What an emitted frame reads back.
impl Screen {
    /// Returns the grid size.
    pub fn grid_size(&self) -> GridSize {
        self.grid.size()
    }

    /// The write cursor as an emitted frame carries it.
    ///
    /// The screen supplies the position; `text_cursor` supplies every
    /// presentation field.
    pub fn cursor(&self, text_cursor: TextCursorModes) -> Cursor {
        Cursor {
            point: GridPoint {
                line: GridLine::from(self.state.line),
                column: self.state.column,
            },
            shape: text_cursor.shape,
            blinking: matches!(text_cursor.blink, CursorBlink::Blinking),
            visible: matches!(text_cursor.enable, TextCursorEnable::Shown),
        }
    }

    /// The cursor position as a `CSI 6 n` report carries it: 1-based,
    /// and relative to the top margin while origin mode confines the
    /// cursor to the scroll region.
    pub fn cursor_position_report(&self) -> (u16, u16) {
        let origin = self.addressable_origin();
        let row = self.state.line.0.saturating_sub(origin.0) + 1;
        let column = self.state.column.0 + 1;
        (row, column)
    }

    /// The cell range the active selection covers, in active-grid
    /// coordinates, widened so that a partly covered wide glyph is
    /// covered whole; `None` without an active selection, when its rows
    /// have left the ring, or when it covers no cell.
    ///
    /// A whole-line selection is not widened.
    pub fn selection_range(&self) -> Option<SelectionRange> {
        let mut range = match self
            .selection
            .resolve(|id| self.grid.grid_line(id), self.grid.size().cols)
        {
            Resolved::Range(range) => range,
            Resolved::None | Resolved::Empty => return None,
        };
        if range.geometry != SelectionGeometry::Lines {
            self.snap_to_glyphs(&mut range);
        }
        Some(range)
    }

    /// The id of the row the cursor sits on.
    pub fn cursor_line_id(&self) -> LineId {
        self.grid.line_id(self.state.line)
    }

    /// The cursor's column.
    pub fn cursor_column(&self) -> GridColumn {
        self.state.column
    }

    /// The cursor's and then the saved cursor's row, column, and whether
    /// each has its deferred wrap armed.
    #[cfg(test)]
    pub fn cursors(&self) -> [(ScreenLine, GridColumn, bool); 2] {
        [
            (self.state.line, self.state.column, self.state.pending_wrap),
            (
                self.checkpoint.line,
                self.checkpoint.column,
                self.checkpoint.pending_wrap,
            ),
        ]
    }

    /// The grid this screen draws on.
    #[cfg(test)]
    pub(crate) fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Moves `range`'s start off a continuation column onto the wide
    /// body to its left, and its end off a wide body onto the
    /// continuation column to its right.
    ///
    /// # Panics
    ///
    /// Panics when either endpoint's line is outside the ring.
    // TODO: Snap each row in `SelectionRange::span_on` when a `Block` or
    // a multi-line `Semantic` selection is implemented; endpoint snapping
    // is sufficient only for `Linear` selection.
    fn snap_to_glyphs(&self, range: &mut SelectionRange) {
        let start = &self.grid.row(range.start.line)[range.start.column];
        if start.width == CellWidth::Spacer {
            debug_assert!(
                range.start.column.0 > 0,
                "a continuation column has a body to its left"
            );
            range.start.column = GridColumn(range.start.column.0.saturating_sub(1));
        }
        let end = &self.grid.row(range.end.line)[range.end.column];
        if end.width == CellWidth::Wide {
            range.end.column = GridColumn(range.end.column.0 + 1);
        }
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
    /// - `DECSC` (`ESC 7`)
    /// - `SCOSC` (`CSI s`)
    pub fn save_checkpoint(&mut self) {
        self.checkpoint = self.capture_checkpoint();
    }

    /// Applies the state saved in memory to each actual state.
    /// If no saved state exists, it performs a DECRC-compliant action.
    ///
    /// The saved position is put back verbatim. Restoring an origin mode
    /// whose margins moved in between can therefore seat the cursor
    /// outside them.
    ///
    /// # Control Functions
    ///
    /// - `DECRC` (`ESC 8`)
    /// - `SCORC` (`CSI u`)
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
    /// way. A selection the reset drops also reports `Full`, even on a
    /// blank grid.
    ///
    /// # Invariants
    ///
    /// The cursor lands at the screen's upper-left corner whatever
    /// origin mode was in force.
    ///
    /// Every placement on this screen stays in the table but becomes
    /// evictable: no anchor taken before the reset resolves after it, so
    /// the next [`Self::evict_lost_anchors`] names every placement.
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
        self.character_set_mapping.reset();
        self.checkpoint = Checkpoint::default();
        dirty.then_some(DamageSpan::Full)
    }

    /// Returns the screen-scoped state a soft reset names to its
    /// power-up value.
    ///
    /// Covers the scrolling margins, the cursor origin, the character
    /// set mapping, the SGR pen and the saved cursor. The margins and
    /// the origin return to their defaults without seating the cursor
    /// at the resulting home.
    ///
    /// The cells, the cursor position, the deferred wrap, the
    /// tabulation stops, the selection and the placements are left as
    /// they are.
    ///
    /// # Control Functions
    ///
    /// - `DECSTR` (`CSI ! p`) — its screen-scoped actions
    pub fn soft_reset(&mut self) {
        self.scroll_region = ScrollRegion::new(self.grid.size().rows);
        self.character_set_mapping.reset();
        self.state.pen = Pen::default();
        self.checkpoint = Checkpoint::default();
    }

    /// Resizes the grid, truncating rather than reflowing; `None` when
    /// the dimensions already matched.
    ///
    /// A shrink pushes as many rows off the top as it takes to keep the
    /// cursor on screen and drops the rest from the bottom. A growth
    /// reclaims rows from history before it appends blank ones. A resize
    /// that changes the dimensions also clears the cursor's landing
    /// cell, since the grid moved under it.
    ///
    /// # Invariants
    ///
    /// A resize that changes the dimensions reports [`DamageSpan::Full`].
    ///
    /// The cursor and the saved cursor both land inside the new grid.
    ///
    /// The saved cursor follows the rows a resize moves exactly as the
    /// live one does, so a `DECRC` after the resize — the one
    /// `DECRST 1049` performs on the way back from the alternate screen
    /// included — lands on the row `DECSC` saved rather than the rows
    /// the resize reclaimed above it. A never-saved checkpoint drifts
    /// off the home position by the same amount.
    ///
    /// A height change returns the margins to the whole page.
    ///
    /// A scrolled-back viewport tracks the rows it was showing.
    pub fn resize(&mut self, size: GridSize) -> Option<DamageSpan> {
        let old = self.grid.size();
        if old == size {
            return None;
        }
        self.state.last_landing = None;
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

    /// Resizes the grid to `size`, rewrapping its rows at the new width
    /// and carrying every position that points into them to the same
    /// text; `None` when the dimensions already matched.
    ///
    /// Under [`ScrollbackOnGrow::Reclaim`], the rows a height growth adds
    /// come back from history, and so do the rows a rewrap frees while the
    /// text or the cursor reached the bottom row; under
    /// [`ScrollbackOnGrow::Keep`] they stay blank. The cursor, the saved
    /// cursor, the selection's ends, the placement anchors, and a
    /// scrolled-back viewport follow the text they stood on; an armed
    /// deferred wrap survives when the cursor lands on a row's right edge,
    /// and a change of height alone leaves the cursor's deferred wrap as it
    /// was. A whole-line selection keeps the whole rows it covered, and a
    /// selection end on the right edge of a row that ends its logical line
    /// stays on the right edge of that line's last row. The cursor and the
    /// saved cursor stay on the screen. A selection with an end on a
    /// dropped row is cleared, a placement whose anchor row is dropped
    /// stops resolving so the next [`Self::evict_lost_anchors`] names it,
    /// and a viewport whose top row is dropped moves to the oldest history
    /// row.
    ///
    /// # Invariants
    ///
    /// A resize that changes the dimensions reports [`DamageSpan::Full`].
    ///
    /// A height change returns the margins to the whole page.
    pub fn reflow(&mut self, size: GridSize, policy: ScrollbackOnGrow) -> Option<DamageSpan> {
        let old = self.grid.size();
        if old == size {
            return None;
        }
        self.state.last_landing = None;
        let mut cursor = TrackedPoint::cursor(
            self.state.line,
            self.state.column,
            self.state.pending_wrap,
            old.cols,
        );
        let mut saved = TrackedPoint::cursor(
            self.checkpoint.line,
            self.checkpoint.column,
            self.checkpoint.pending_wrap,
            old.cols,
        );
        let anchors = self.webview_placements.anchors();
        let mut riders = Riders::gather(
            &self.grid,
            &self.selection,
            &anchors,
            self.viewport.offset.0,
            old.cols,
        );
        let armed = self.state.pending_wrap;
        self.grid
            .reflow(&mut cursor, &mut saved, &mut riders.points, size, policy);
        self.seat_reflowed(cursor, saved, size);
        if old.cols == size.cols {
            self.state.pending_wrap = armed;
        }
        self.land_riders(&riders, &anchors, size.cols);
        if old.rows != size.rows {
            self.scroll_region.set_margins(Margins::new(size.rows));
        }
        Some(DamageSpan::Full)
    }

    /// Fills the visible screen with the alignment pattern, returning to
    /// the page-wide scroll region and the absolute cursor origin.
    ///
    /// The pattern is drawn with default attributes rather than the
    /// current pen.
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
                .fill_visible_row_range(ScreenLine(line), 0..size.cols, cell.clone());
        }
        self.scroll_region = ScrollRegion::new(size.rows);
        self.seat_cursor(ScreenLine(0), GridColumn(0));
        DamageSpan::Full
    }

    /// Seats the cursor and the saved cursor on the positions a reflow
    /// carried them to, clamped onto a screen of `size`.
    fn seat_reflowed(&mut self, cursor: TrackedPoint, saved: TrackedPoint, size: GridSize) {
        let last_line = size.rows.saturating_sub(1);
        (self.state.line, self.state.column, self.state.pending_wrap) =
            Self::cursor_at(cursor, size.cols, last_line);
        (
            self.checkpoint.line,
            self.checkpoint.column,
            self.checkpoint.pending_wrap,
        ) = Self::cursor_at(saved, size.cols, last_line);
    }

    /// Moves the selection, the placements and a scrolled-back viewport onto
    /// the positions a reflow to `cols` columns carried `riders` to, with
    /// `anchors` the placements' anchors from before it.
    ///
    /// A selection end that stood on the right edge of a row ending its
    /// logical line lands on the right edge again. A selection with an end
    /// on a dropped row is cleared, a placement whose anchor row is dropped
    /// moves to a retired id, and a viewport whose top row is dropped moves
    /// to the oldest history row.
    fn land_riders(&mut self, riders: &Riders, anchors: &[(LineId, GridColumn)], cols: u16) {
        if let Some(ends) = riders.selection {
            self.land_selection(riders, ends, cols);
        }
        self.land_placements(riders, anchors, cols);
        if let Some(index) = riders.viewport {
            self.land_viewport(riders.moved(index));
        }
    }

    /// Moves the selection onto the positions a reflow to `cols` columns
    /// carried its `ends` in `riders` to, or clears it when the row of
    /// either end is gone.
    ///
    /// An end that stood on the right edge of a row ending its logical line
    /// lands on the right edge again.
    fn land_selection(&mut self, riders: &Riders, ends: [(usize, bool); 2], cols: u16) {
        let ends = ends.map(|(index, on_line_end_edge)| {
            riders.moved(index).and_then(|point| {
                self.grid.line_id_at(point.line()).map(|line| SelectionEnd {
                    line,
                    boundary: if on_line_end_edge {
                        cols
                    } else {
                        point.boundary().min(cols)
                    },
                })
            })
        });
        match ends {
            [Some(anchor), Some(moving)] => self.selection.relocate(anchor, moving),
            _ => {
                let _ = self.selection.clear();
            }
        }
    }

    /// Re-anchors each placement on the position a reflow to `cols`
    /// columns carried its anchor in `riders` to, with `anchors` the
    /// placements' anchors from before it; a placement whose anchor row is
    /// gone moves to a retired id.
    fn land_placements(&mut self, riders: &Riders, anchors: &[(LineId, GridColumn)], cols: u16) {
        let retired = self.grid.retired_id();
        let last_column = cols.saturating_sub(1);
        let reanchored: Vec<(LineId, GridColumn)> = riders
            .anchors
            .clone()
            .zip(anchors)
            .map(|(index, (_, column))| {
                riders
                    .moved(index)
                    .and_then(|point| {
                        self.grid
                            .line_id_at(point.line())
                            .map(|line| (line, GridColumn(point.boundary().min(last_column))))
                    })
                    .unwrap_or((retired, *column))
            })
            .collect();
        self.webview_placements.reanchor(&reanchored);
    }

    /// Scrolls the viewport back to show the row a reflow carried its top
    /// row to, `top`, on top; to the oldest history row when that row is
    /// gone.
    fn land_viewport(&mut self, top: Option<TrackedPoint>) {
        let offset = match top {
            Some(point) => u32::try_from(point.line().0.saturating_neg()).unwrap_or(0),
            None => u32::MAX,
        };
        self.set_display_offset(DisplayOffset(offset));
    }

    /// The cursor position `point` stands for on a screen `cols` wide
    /// whose last row is `last_line`: a point on the right edge parks on
    /// the last column with the deferred wrap armed.
    fn cursor_at(point: TrackedPoint, cols: u16, last_line: u16) -> (ScreenLine, GridColumn, bool) {
        let line = ScreenLine(u16::try_from(point.line().0).unwrap_or(0).min(last_line));
        if point.boundary() >= cols {
            (line, GridColumn(cols.saturating_sub(1)), true)
        } else {
            (line, GridColumn(point.boundary()), false)
        }
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
impl Screen {
    /// Registers a mount at the write cursor under the id the host minted.
    pub fn mount_placement(&mut self, id: InstanceId, size: PlacementSize) {
        let anchor = self.cursor_line_id();
        let col = self.cursor_column();
        self.webview_placements.mount(id, anchor, col, size);
    }

    /// Registers a mount anchored at the visible row `row` and column
    /// `column` under the id the host minted.
    ///
    /// `row` and `column` must lie inside the grid; the caller must
    /// bounds-check them against [`Self::grid_size`].
    pub fn mount_placement_at(
        &mut self,
        id: InstanceId,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
    ) {
        let anchor = self.grid.line_id(row);
        self.webview_placements.mount(id, anchor, column, size);
    }

    /// Drops the placement a re-mount replaces, without reporting it.
    pub fn supersede_placement(&mut self, id: InstanceId) {
        self.webview_placements.supersede(id);
    }

    /// Removes the placement a client `unmount` addresses (`None`
    /// removes every placement on this screen); returns whether
    /// anything went.
    pub fn unmount_placement(&mut self, id: Option<InstanceId>) -> bool {
        self.webview_placements.unmount(id)
    }

    /// Removes the placements the host names; returns whether anything
    /// went.
    pub fn remove_placements(&mut self, ids: &[InstanceId]) -> bool {
        self.webview_placements.remove_many(ids)
    }

    /// Empties this screen's table and names every id it held.
    pub fn take_placements(&mut self) -> Vec<InstanceId> {
        self.webview_placements.take_all()
    }

    /// Number of placements this screen holds.
    pub fn placement_count(&self) -> usize {
        self.webview_placements.len()
    }

    /// Resolves this screen's placements into grid coordinates.
    ///
    /// # Invariants
    ///
    /// A placement this omits is exactly one [`Self::evict_lost_anchors`]
    /// evicts.
    pub fn project_placements(&self) -> Vec<AnchoredPlacement> {
        self.webview_placements
            .project(|anchor| self.grid.grid_line(anchor))
    }

    /// Drops the placements whose anchor row left this screen's grid and
    /// names them.
    pub fn evict_lost_anchors(&mut self) -> Vec<InstanceId> {
        self.webview_placements
            .evict_lost_anchors(|anchor| self.grid.grid_line(anchor))
    }
}

/// The selection this screen owns.
///
/// A selection can only ever be resolved against the grid that minted
/// its rows.
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
    /// whether it moved. It is a no-op without an active selection or for
    /// a cell outside the grid.
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
    /// Each row's span comes from [`SelectionRange::span_on`]. Rows are
    /// joined by `\n` with none after the last, except that a row whose
    /// logical line continues on the next row joins it directly; its cells
    /// past the recorded wrap contribute nothing. Trailing blanks are
    /// trimmed where a logical line ends and at the end of the selection,
    /// never at a soft wrap. A continuation column and a wrap filler
    /// contribute nothing, and a cell's combining marks follow its glyph.
    pub fn selection_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        let last_column = self.grid.size().cols - 1;
        let mut text = String::new();
        let mut logical_line = String::new();
        for line in range.start.line.0..=range.end.line.0 {
            let (first, last) = range.span_on(line, last_column);
            let grid_line = GridLine(line);
            self.push_span_text(&mut logical_line, grid_line, first..=last);
            let line_ends = self.grid.wrap_at(grid_line).is_none();
            if line_ends && line != range.end.line.0 {
                text.push_str(logical_line.trim_end());
                text.push('\n');
                logical_line.clear();
            }
        }
        text.push_str(logical_line.trim_end());
        Some(text)
    }

    /// The endpoint a host cell stands for; `None` when the cell is
    /// outside the ring or past the width.
    fn selection_end(&self, cell: GridPoint, side: CellSide) -> Option<SelectionEnd> {
        let line = self.grid.line_id_at_point(cell)?;
        Some(SelectionEnd::at(line, cell.column, side))
    }

    /// Appends to `out` the text of the cells in `columns` of the row at
    /// `line`; the cells past the row's recorded wrap contribute nothing.
    fn push_span_text(&self, out: &mut String, line: GridLine, columns: RangeInclusive<u16>) {
        let wrap = self.grid.wrap_at(line);
        let row = self.grid.row(line);
        out.extend(
            columns
                .filter(|column| wrap.is_none_or(|cells| *column < cells))
                .map(|column| &row[GridColumn(column)])
                .flat_map(Cell::chars),
        );
    }
}

/// The damage one print reports.
struct PrintDamage {
    /// The first row the print touches.
    first_line: ScreenLine,
    /// The row the glyph lands on.
    last_line: ScreenLine,
    /// Whether a wrap scrolled the screen.
    scrolled: bool,
}

impl PrintDamage {
    /// Reports [`DamageSpan::Full`] when a wrap scrolled, otherwise the
    /// rows from the first one the print touched through the landing row,
    /// or `None` when those rows have scrolled out of the window.
    fn span(self, screen: &Screen) -> Option<DamageSpan> {
        if self.scrolled {
            return Some(DamageSpan::Full);
        }
        screen.damage_span(self.first_line, self.last_line)
    }
}

/// The positions a reflow carries besides the two cursors, and what each
/// one stands for.
struct Riders {
    /// Every carried position, at the indices the fields below name.
    points: Vec<Option<TrackedPoint>>,
    /// The selection's anchor and moving ends, each as its index in
    /// `points` and whether it stood on the right edge of a row that ends
    /// its logical line; `None` without a selection.
    selection: Option<[(usize, bool); 2]>,
    /// The indices in `points` of the placement anchors, in the order the
    /// placements list them.
    anchors: Range<usize>,
    /// The index in `points` of the top row a scrolled-back viewport shows;
    /// `None` at the live tail.
    viewport: Option<usize>,
}

impl Riders {
    /// Gathers what a reflow of `grid` from `cols` columns carries: the ends
    /// of `selection`, the placement `anchors`, and the top row of a
    /// viewport scrolled back by `viewport` rows.
    ///
    /// A whole-line selection is carried from the left edge of its top row
    /// to the right edge of its bottom row.
    fn gather(
        grid: &Grid,
        selection: &ScreenSelection,
        anchors: &[(LineId, GridColumn)],
        viewport: u32,
        cols: u16,
    ) -> Self {
        let mut riders = Self {
            points: Vec::with_capacity(anchors.len() + 3),
            selection: None,
            anchors: 0..0,
            viewport: None,
        };
        riders.carry_selection(grid, selection, cols);
        riders.carry_anchors(grid, anchors);
        riders.carry_viewport(viewport);
        riders
    }

    /// Where the position at `index` in `points` stands now; `None` once
    /// its row is gone.
    fn moved(&self, index: usize) -> Option<TrackedPoint> {
        self.points.get(index).copied().flatten()
    }

    /// Carries the ends of `selection` on `grid`, a grid `cols` wide.
    ///
    /// A whole-line selection is carried from the left edge of its top row
    /// to the right edge of its bottom row.
    fn carry_selection(&mut self, grid: &Grid, selection: &ScreenSelection, cols: u16) {
        let Some((anchor, moving)) = selection.ends() else {
            return;
        };
        let lines = [anchor, moving].map(|end| grid.grid_line(end.line));
        let anchor_on_top = lines[0].map(|line| line.0) <= lines[1].map(|line| line.0);
        let boundaries = match selection.kind() {
            Some(SelectionKind::Lines) if anchor_on_top => [0, cols],
            Some(SelectionKind::Lines) => [cols, 0],
            _ => [anchor.boundary, moving.boundary],
        };
        self.selection = Some([
            self.carry_selection_end(grid, lines[0], boundaries[0], cols),
            self.carry_selection_end(grid, lines[1], boundaries[1], cols),
        ]);
    }

    /// Carries a selection end at `boundary` on `line` of `grid`, a grid
    /// `cols` wide, returning its index in `self.points` and whether it
    /// stands on the right edge of a row that ends its logical line.
    fn carry_selection_end(
        &mut self,
        grid: &Grid,
        line: Option<GridLine>,
        boundary: u16,
        cols: u16,
    ) -> (usize, bool) {
        let on_line_end_edge =
            boundary >= cols && line.is_some_and(|line| grid.wrap_at(line).is_none());
        let index = self.carry(line.map(|line| TrackedPoint::new(line, boundary)));
        (index, on_line_end_edge)
    }

    /// Carries each of the placement `anchors` on `grid`, in order.
    fn carry_anchors(&mut self, grid: &Grid, anchors: &[(LineId, GridColumn)]) {
        let first = self.points.len();
        self.points.extend(anchors.iter().map(|(line, column)| {
            grid.grid_line(*line)
                .map(|line| TrackedPoint::new(line, column.0))
        }));
        self.anchors = first..self.points.len();
    }

    /// Carries the top row of a viewport scrolled back by `viewport` rows;
    /// nothing at the live tail.
    fn carry_viewport(&mut self, viewport: u32) {
        if viewport == 0 {
            return;
        }
        let top = i32::try_from(viewport)
            .ok()
            .map(|offset| TrackedPoint::new(GridLine(-offset), 0));
        self.viewport = Some(self.carry(top));
    }

    /// Adds `point` to the carried positions, returning its index in
    /// `self.points`.
    fn carry(&mut self, point: Option<TrackedPoint>) -> usize {
        self.points.push(point);
        self.points.len() - 1
    }
}

#[cfg(test)]
mod tests;
