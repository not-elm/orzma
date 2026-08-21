//! Alacritty-backed [`OldOrzmaVt`] implementation.

use crate::{
    damage::StagedDamage,
    schema::{
        CellSide, Cursor, GridPoint, GridSize, MouseEncoding, MouseTracking, Palette, ScreenKind,
        Scroll, SelectionKind, SelectionRange, ViCursor, ViModeSwitch, VtModes, VtResult, VtSignal,
    },
    screen::viewport::DisplayOffset,
    vt::{VtBackend, VtSelection, apc::ApcState},
};
use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Point, Side},
    selection::Selection,
    term::{Config, TermMode},
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

    fn interpret(&mut self, chunk: &[u8]) -> Option<StagedDamage> {
        if chunk.is_empty() {
            return None;
        }
        self.apc_parser.parse(chunk, &mut self.apc_state);
        self.processor.advance(&mut self.term, chunk);
        let damage = StagedDamage::from_alacritty_term(&mut self.term);
        self.term.reset_damage();
        Some(damage)
    }

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_ {
        // TODO: drain control frames captured from the APC stream.
        iter::empty()
    }

    fn drain_replies_into(&self, _buf: &mut Vec<u8>) {
        // TODO: Buffer DSR/DA replies from the interpreter and drain
        // them here; the backend produces none yet, so there is
        // nothing to copy.
    }

    fn scroll(&mut self, scroll: Scroll) -> Option<StagedDamage> {
        let prev_offset = self.display_offset();
        let screen_lines = self.grid_size().rows;
        self.term
            .scroll_display(scroll.to_alacritty_scroll(screen_lines));
        if self.display_offset() == prev_offset {
            return None;
        }
        self.term.reset_damage();
        Some(StagedDamage::Full)
    }

    fn modes(&self) -> VtModes {
        let mode = self.term.mode();
        VtModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            active_screen: if mode.contains(TermMode::ALT_SCREEN) {
                ScreenKind::Alternate
            } else {
                ScreenKind::Primary
            },
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            focus_in_out: mode.contains(TermMode::FOCUS_IN_OUT),
            mouse_encoding: MouseEncoding::from_alacritty_term_mode(mode),
            mouse_tracking: MouseTracking::from_alacritty_term_mode(mode),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Option<StagedDamage> {
        if self.grid_size() == (GridSize { cols, rows }) {
            return None;
        }
        self.term.resize(LocalDim::new(cols, rows));
        self.term.reset_damage();
        Some(StagedDamage::Full)
    }

    #[inline]
    fn grid_size(&self) -> GridSize {
        GridSize {
            cols: self.term.columns() as u16,
            rows: self.term.screen_lines() as u16,
        }
    }

    fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult<Option<StagedDamage>> {
        let in_vi_mode = self.term.mode().contains(TermMode::VI);
        let transitions = !in_vi_mode && vi_mode == ViModeSwitch::Enter
            || in_vi_mode && vi_mode == ViModeSwitch::Exit;
        if !transitions {
            return Ok(None);
        }
        self.term.toggle_vi_mode();
        Ok(Some(StagedDamage::Full))
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
    ) -> VtResult<Option<StagedDamage>> {
        let point = Point::from(cell);
        let side = Side::from(side);
        let mut selection = Selection::new(kind.into(), point, side);
        selection.update(point, side.opposite());
        self.term.selection = Some(selection);
        Ok(Some(StagedDamage::Full))
    }

    fn start_selection_at_vi_cursor(
        &mut self,
        kind: SelectionKind,
    ) -> VtResult<Option<StagedDamage>> {
        let cursor_point = self.term.vi_mode_cursor.point;
        let mut selection = Selection::new(kind.into(), cursor_point, Side::Left);
        selection.update(cursor_point, Side::Left.opposite());
        self.term.selection = Some(selection);
        Ok(Some(StagedDamage::Full))
    }

    fn update_selection(
        &mut self,
        cell: GridPoint,
        side: CellSide,
    ) -> VtResult<Option<StagedDamage>> {
        let point = Point::from(cell);
        let side = Side::from(side);
        Ok(self.term.selection.as_mut().map(|selection| {
            selection.update(point, side);
            StagedDamage::Full
        }))
    }

    fn change_selection_kind(&mut self, kind: SelectionKind) -> VtResult<Option<StagedDamage>> {
        let vi_point = self.term.vi_mode_cursor.point;
        Ok(self.term.selection.as_mut().map(|selection| {
            selection.ty = kind.into();
            selection.update(vi_point, Side::Left);
            selection.include_all();
            StagedDamage::Full
        }))
    }

    fn clear_selection(&mut self) -> VtResult<Option<StagedDamage>> {
        Ok(self.term.selection.take().map(|_| StagedDamage::Full))
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

#[cfg(test)]
mod tests;
