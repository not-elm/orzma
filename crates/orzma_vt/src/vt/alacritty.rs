//! Alacritty-backed [`OrzmaVt`] implementation.

use crate::{
    schema::{
        Damage, DamageVerdict, Frame, MouseEncoding, MouseTracking, Scroll, SelectionKind,
        SelectionOp, SelectionRange, ViModeSwitch, ViewportPoint, VtModes, VtResult, VtSignal,
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

pub struct AlacrittyVt {
    processor: Processor,
    term: Term<OrzmaTermEventHandler>,
    apc_state: ApcState,
    apc_parser: VTParser,
    /// Damage staged for the next [`OrzmaVt::frames`] call.
    ///
    /// Merged rather than replaced on each stage. Alacritty accumulates
    /// line damage internally until `Term::reset_damage`, so replacing
    /// would be safe for the damage it tracks — but it excludes
    /// selection state, so a repaint staged by a selection change is
    /// only ever held here and would be lost to the next chunk.
    pending_damage: Option<Damage>,
}

impl VtBackend for AlacrittyVt {
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
            pending_damage: Some(Damage::Full),
        }
    }

    #[inline]
    fn display_offset(&self) -> u32 {
        self.term.grid().display_offset() as u32
    }

    fn interpret(&mut self, chunk: &[u8]) -> Option<DamageVerdict> {
        if chunk.is_empty() {
            return None;
        }
        self.apc_parser.parse(chunk, &mut self.apc_state);
        self.processor.advance(&mut self.term, chunk);
        let damage = Damage::from_alacritty_term(&mut self.term);
        let verdict = DamageVerdict::classify(&damage);
        self.stage_damage(damage);
        Some(verdict)
    }

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_ {
        // TODO: drain control frames captured from the APC stream.
        iter::empty()
    }

    fn drain_replies_into(&self, _buf: &mut Vec<u8>) {
        todo!()
    }

    #[inline]
    fn scroll(&mut self, scroll: Scroll) {
        let screen_lines = self.term.screen_lines() as u16;
        self.term
            .scroll_display(scroll.to_alacritty_scroll(screen_lines));
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

    fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(LocalDim::new(cols, rows));
        self.stage_damage(Damage::Full);
    }

    #[inline]
    fn grid_size(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }

    fn apply_selection(&mut self, op: SelectionOp) -> VtResult {
        match op {
            SelectionOp::StartAt { cell, side, kind } => {
                let point = self.grid_point(cell);
                let side = Side::from(side);
                let mut selection = Selection::new(kind.into(), point, side);
                selection.update(point, side.opposite());
                self.term.selection = Some(selection);
                self.stage_damage(Damage::Full);
            }
            SelectionOp::StartAtViCursor { kind } => {
                let cursor_point = self.term.vi_mode_cursor.point;
                let selection = Selection::new(kind.into(), cursor_point, Side::Left);
                self.term.selection.replace(selection);
                self.stage_damage(Damage::Full);
            }
            SelectionOp::UpdateTo { cell, side } => {
                let point = self.grid_point(cell);
                if let Some(selection) = self.term.selection.as_mut() {
                    let s: Side = side.into();
                    selection.update(point, s);
                }
            }
            SelectionOp::ChangeKind(selection_kind) => {
                let vi_point = self.term.vi_mode_cursor.point;
                let Some(selection) = self.term.selection.as_mut() else {
                    return Ok(());
                };
                selection.ty = selection_kind.into();
                selection.update(vi_point, Side::Left);
                selection.include_all();
                self.stage_damage(Damage::Full);
            }
            SelectionOp::Clear => {
                self.term.selection.take();
            }
        }
        Ok(())
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

    fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult {
        let prev = *self.term.mode();
        if prev.contains(TermMode::VI) && vi_mode == ViModeSwitch::Enter
            || !prev.contains(TermMode::VI) && vi_mode == ViModeSwitch::Exit
        {
            self.term.toggle_vi_mode();
        }
        Ok(())
    }
}

impl AlacrittyVt {
    /// Resolves a viewport cell onto the grid row it currently sits on.
    ///
    /// alacritty's `Line` counts from the top of the active screen area
    /// and goes negative into scrollback, so the display offset has to
    /// come off the viewport row. Rows outside the viewport are kept as
    /// they arrive rather than clamped: a drag that leaves the viewport
    /// names a real scrollback row, and `Selection::to_range` clamps to
    /// the grid on its own.
    fn grid_point(&self, cell: ViewportPoint) -> Point {
        let display_offset = self.term.grid().display_offset() as i32;
        Point::new(
            Line(i32::from(cell.row) - display_offset),
            Column(usize::from(cell.column)),
        )
    }

    /// Merges `damage` into the value staged for the next
    /// [`OrzmaVt::frames`] call.
    ///
    /// Seeding an absent staged value with an empty row set is safe
    /// because that set is the merge identity.
    fn stage_damage(&mut self, damage: Damage) {
        *self.pending_damage.get_or_insert(Damage::Rows(Vec::new())) |= damage;
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
