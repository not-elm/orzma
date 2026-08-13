//! Alacritty-backed [`OrzmaVt`] implementation.

use crate::{
    schema::{
        Damage, DisplayOffset, Frame, GridSize, MouseEncoding, MouseTracking, Scroll,
        SelectionKind, SelectionOp, SelectionRange, ViModeSwitch, ViewportPoint, VtModes, VtResult,
        VtSignal,
    },
    vt::{VtBackend, apc::ApcState},
};
use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line, Point, Side},
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
        // NOTE: `scroll_display` just latched full damage in alacritty's
        // accumulator. Resetting converts that mark into this return
        // value; the accumulator holds nothing else because `interpret`
        // resets after every read.
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
        // NOTE: same recipe as `scroll` — `Term::resize` latched full
        // damage; the reset converts it into this return value.
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

    fn apply_selection(&mut self, op: SelectionOp) -> VtResult<Option<Damage>> {
        let damage = match op {
            SelectionOp::StartAt { cell, side, kind } => {
                let point = self.grid_point(cell);
                let side = Side::from(side);
                let mut selection = Selection::new(kind.into(), point, side);
                selection.update(point, side.opposite());
                self.term.selection = Some(selection);
                Some(Damage::Full)
            }
            SelectionOp::StartAtViCursor { kind } => {
                let cursor_point = self.term.vi_mode_cursor.point;
                let selection = Selection::new(kind.into(), cursor_point, Side::Left);
                self.term.selection.replace(selection);
                Some(Damage::Full)
            }
            SelectionOp::UpdateTo { cell, side } => {
                let point = self.grid_point(cell);
                match self.term.selection.as_mut() {
                    Some(selection) => {
                        let s: Side = side.into();
                        selection.update(point, s);
                        Some(Damage::Full)
                    }
                    None => None,
                }
            }
            SelectionOp::ChangeKind(selection_kind) => {
                let vi_point = self.term.vi_mode_cursor.point;
                match self.term.selection.as_mut() {
                    Some(selection) => {
                        selection.ty = selection_kind.into();
                        selection.update(vi_point, Side::Left);
                        selection.include_all();
                        Some(Damage::Full)
                    }
                    None => None,
                }
            }
            SelectionOp::Clear => self.term.selection.take().map(|_| Damage::Full),
        };
        Ok(damage)
    }

    fn selection_range(&self) -> Option<SelectionRange> {
        let display_offset = self.display_offset();
        let selection = self.term.selection.as_ref()?;
        let selection_kind: SelectionKind = selection.ty.into();
        let range = selection.to_range(&self.term)?;
        Some(SelectionRange {
            start: ViewportPoint::from_alacritty_point(range.start, display_offset),
            end: ViewportPoint::from_alacritty_point(range.end, display_offset),
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
}

impl AlacrittyVtBackend {
    /// Resolves a viewport cell onto the grid row it currently sits on.
    ///
    /// alacritty's `Line` counts from the top of the active screen area
    /// and goes negative into scrollback, so the display offset has to
    /// come off the viewport row. Rows outside the viewport are kept as
    /// they arrive rather than clamped: a drag that leaves the viewport
    /// names a real scrollback row, and `Selection::to_range` clamps to
    /// the grid on its own.
    fn grid_point(&self, cell: ViewportPoint) -> Point {
        let display_offset = self.display_offset().0 as i32;
        Point::new(
            Line(i32::from(cell.row) - display_offset),
            Column(usize::from(cell.column)),
        )
    }
}

struct OrzmaTermEventHandler {}

impl EventListener for OrzmaTermEventHandler {
    fn send_event(&self, _event: alacritty_terminal::event::Event) {}
}

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
