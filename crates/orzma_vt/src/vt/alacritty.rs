//! Alacritty-backed [`OldOrzmaVt`] implementation.

use crate::{
    schema::{
        CellSide, Cursor, Damage, DamageRows, DisplayOffset, GridCell, GridPoint, GridSize,
        MouseEncoding, MouseTracking, Palette, Scroll, SelectionKind, SelectionRange, ViCursor,
        ViModeSwitch, VtModes, VtResult, VtSignal, style,
    },
    vt::{VtBackend, VtSelection, apc::ApcState},
};
use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::{Dimensions, Row},
    index::{Column, Line, Point, Side},
    selection::Selection,
    term::{
        Config, TermMode,
        cell::{Cell, Flags},
    },
    vte::ansi::Processor,
};
use std::iter;
use vtparse::VTParser;

pub struct AlacrittyVtBackend {
    processor: Processor,
    term: Term<OrzmaTermEventHandler>,
    apc_state: ApcState,
    apc_parser: VTParser,
}

impl VtBackend for AlacrittyVtBackend {
    /// Builds a VT backed by an alacritty `Term` at the given grid size.
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            processor: Processor::new(),
            term: Term::new(
                Config::default(),
                &LocalDim::new(cols, rows),
                OrzmaTermEventHandler {},
            ),
            apc_state: ApcState::default(),
            apc_parser: VTParser::new(),
        }
    }

    #[inline]
    fn display_offset(&self) -> DisplayOffset {
        DisplayOffset(self.term.grid().display_offset() as u32)
    }

    fn interpret(&mut self, chunk: &[u8]) -> Option<Damage> {
        if chunk.is_empty() {
            return None;
        }
        self.apc_parser.parse(chunk, &mut self.apc_state);
        self.processor.advance(&mut self.term, chunk);
        let damage = Damage::from_alacritty_term(&mut self.term);
        self.term.reset_damage();
        Some(damage)
    }

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_ {
        // TODO: drain control frames captured from the APC stream.
        iter::empty()
    }

    fn drain_replies_into(&self, _buf: &mut Vec<u8>) {
        todo!()
    }

    fn scroll(&mut self, scroll: Scroll) -> Option<Damage> {
        let prev_offset = self.display_offset();
        let screen_lines = self.grid_size().rows;
        self.term
            .scroll_display(scroll.to_alacritty_scroll(screen_lines));
        if self.display_offset() == prev_offset {
            return None;
        }
        self.term.reset_damage();
        Some(Damage::Full)
    }

    fn modes(&self) -> VtModes {
        let mode = self.term.mode();
        VtModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            focus_in_out: mode.contains(TermMode::FOCUS_IN_OUT),
            mouse_encoding: MouseEncoding::from_alacritty_term_mode(mode),
            mouse_tracking: MouseTracking::from_alacritty_term_mode(mode),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Option<Damage> {
        if self.grid_size() == (GridSize { cols, rows }) {
            return None;
        }
        self.term.resize(LocalDim::new(cols, rows));
        self.term.reset_damage();
        Some(Damage::Full)
    }

    #[inline]
    fn grid_size(&self) -> GridSize {
        GridSize {
            cols: self.term.columns() as u16,
            rows: self.term.screen_lines() as u16,
        }
    }

    fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult<Option<Damage>> {
        let in_vi_mode = self.term.mode().contains(TermMode::VI);
        let transitions = !in_vi_mode && vi_mode == ViModeSwitch::Enter
            || in_vi_mode && vi_mode == ViModeSwitch::Exit;
        if !transitions {
            return Ok(None);
        }
        self.term.toggle_vi_mode();
        Ok(Some(Damage::Full))
    }

    fn cell_at(&self, point: GridPoint) -> Option<GridCell> {
        let grid = self.term.grid();
        let history = grid.history_size() as i32;
        let lines = -history..self.term.screen_lines() as i32;
        if !lines.contains(&point.line.0) || usize::from(point.column.0) >= self.term.columns() {
            return None;
        }
        let cell = &grid[Line(point.line.0)][Column(usize::from(point.column.0))];
        Some(GridCell {
            text: cell_text(cell),
            width: cell_width(cell.flags),
            point,
            fg: cell.fg.into(),
            bg: cell.bg.into(),
            style: style_bits(cell.flags),
            // TODO: resolve the hyperlink through the interner once the
            // backend owns one; the id-carrying schema type cannot be
            // built without it.
            hyperlink: None,
        })
    }

    fn palette(&self) -> Palette {
        Palette::from_alacritty_colors(self.term.colors())
    }

    fn cursor(&self) -> Cursor {
        let style = self.term.cursor_style();
        Cursor {
            point: self.term.grid().cursor.point.into(),
            shape: style.shape.into(),
            blinking: style.blinking,
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR)
                && style.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden,
        }
    }

    fn vi_cursor(&self) -> Option<ViCursor> {
        if !self.term.mode().contains(TermMode::VI) {
            return None;
        }
        Some(ViCursor {
            point: self.term.vi_mode_cursor.point.into(),
        })
    }
}

impl VtSelection for AlacrittyVtBackend {
    fn start_selection(
        &mut self,
        cell: GridPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> VtResult<Option<Damage>> {
        let point = Point::from(cell);
        let side = Side::from(side);
        let mut selection = Selection::new(kind.into(), point, side);
        selection.update(point, side.opposite());
        self.term.selection = Some(selection);
        Ok(Some(Damage::Full))
    }

    fn start_selection_at_vi_cursor(&mut self, kind: SelectionKind) -> VtResult<Option<Damage>> {
        let cursor_point = self.term.vi_mode_cursor.point;
        let mut selection = Selection::new(kind.into(), cursor_point, Side::Left);
        selection.update(cursor_point, Side::Left.opposite());
        self.term.selection = Some(selection);
        Ok(Some(Damage::Full))
    }

    fn update_selection(&mut self, cell: GridPoint, side: CellSide) -> VtResult<Option<Damage>> {
        let point = Point::from(cell);
        let side = Side::from(side);
        Ok(self.term.selection.as_mut().map(|selection| {
            selection.update(point, side);
            Damage::Full
        }))
    }

    fn change_selection_kind(&mut self, kind: SelectionKind) -> VtResult<Option<Damage>> {
        let vi_point = self.term.vi_mode_cursor.point;
        Ok(self.term.selection.as_mut().map(|selection| {
            selection.ty = kind.into();
            selection.update(vi_point, Side::Left);
            selection.include_all();
            Damage::Full
        }))
    }

    fn clear_selection(&mut self) -> VtResult<Option<Damage>> {
        Ok(self.term.selection.take().map(|_| Damage::Full))
    }

    fn selection_range(&self) -> Option<SelectionRange> {
        let selection = self.term.selection.as_ref()?;
        let selection_kind: SelectionKind = selection.ty.into();
        let range = selection.to_range(&self.term)?;
        Some(SelectionRange {
            start: range.start.into(),
            end: range.end.into(),
            geometry: selection_kind.into(),
        })
    }

    fn selection_kind(&self) -> Option<SelectionKind> {
        let s = self.term.selection.as_ref()?.ty;
        Some(s.into())
    }

    #[inline]
    fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }
}

struct OrzmaTermEventHandler {}

impl EventListener for OrzmaTermEventHandler {}

/// Grid size handed to `Term::new` / `Term::resize`.
/// Alacritty's own `TermSize` is `pub(crate)`, so a minimal local
/// equivalent lives here. `total_lines == screen_lines` on purpose:
/// scrollback capacity comes from `Config::scrolling_history`, not
/// from the size type.
struct LocalDim {
    cols: usize,
    rows: usize,
}

impl LocalDim {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.into(),
            rows: rows.into(),
        }
    }
}

impl Dimensions for LocalDim {
    fn columns(&self) -> usize {
        self.cols
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn total_lines(&self) -> usize {
        self.rows
    }
}

/// Grapheme cluster for one cell: the primary character plus any
/// zero-width followers.
fn cell_text(cell: &Cell) -> String {
    let mut text = String::new();
    text.push(cell.c);
    if let Some(zerowidth) = cell.zerowidth() {
        text.extend(zerowidth);
    }
    text
}

/// Display width per the wide-char flags: spacers render nothing, wide
/// glyphs span two columns.
fn cell_width(flags: Flags) -> u8 {
    if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
        0
    } else if flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
}

/// Maps alacritty cell flags onto the shared style bitmask.
fn style_bits(flags: Flags) -> u16 {
    let pairs = [
        (Flags::BOLD, style::BOLD),
        (Flags::ITALIC, style::ITALIC),
        (Flags::UNDERLINE, style::UNDERLINE),
        (Flags::STRIKEOUT, style::STRIKE),
        (Flags::INVERSE, style::REVERSE),
        (Flags::DIM, style::DIM),
        (Flags::HIDDEN, style::HIDDEN),
    ];
    pairs
        .into_iter()
        .filter(|(flag, _)| flags.contains(*flag))
        .fold(0, |bits, (_, bit)| bits | bit)
}

#[cfg(test)]
mod tests;
