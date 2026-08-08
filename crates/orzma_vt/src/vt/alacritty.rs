//! Alacritty-backed [`OrzmaVt`] implementation.

use crate::{
    damage::{DamageVerdict, DirtyRows},
    extension::ApcState,
    modes::{MouseEncoding, MouseTracking, VtModes},
    signal::VtSignal,
    vt::OrzmaVt,
};
use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::{Dimensions, Scroll},
    term::{Config, TermDamage, TermMode},
    vte::ansi::{Color, Processor},
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
    /// Replaced rather than unioned on each stage: alacritty accumulates
    /// line damage internally until `Term::reset_damage`, so the newest
    /// read already covers every chunk since the last emit.
    pending_damage: Option<DirtyRows>,
}

impl OrzmaVt for AlacrittyVt {
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
            pending_damage: None,
        }
    }

    fn interpret(&mut self, chunk: &[u8]) -> Option<DamageVerdict> {
        if chunk.is_empty() {
            return None;
        }
        self.apc_parser.parse(chunk, &mut self.apc_state);
        self.processor.advance(&mut self.term, chunk);
        let dirty = DirtyRows::from_alacritty_term(&mut self.term);
        Some(DamageVerdict::classify(&dirty))
    }

    fn frames(&mut self) -> Vec<crate::frame::Frame> {
        todo!()
    }

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_ {
        // TODO: drain control frames captured from the APC stream.
        iter::empty()
    }

    fn drain_replies_into(&self, _buf: &mut Vec<u8>) {
        todo!()
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

    #[inline]
    fn display_offset(&self) -> u32 {
        self.term.grid().display_offset() as u32
    }

    #[inline]
    fn scroll(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    #[inline]
    fn scroll_to_bottom(&mut self) {
        self.scroll(i32::MIN);
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
mod tests {
    use super::*;

    fn vt_after(bytes: &[u8]) -> AlacrittyVt {
        let mut vt = AlacrittyVt::new(80, 24);
        vt.interpret(bytes);
        vt
    }

    // NOTE: alacritty's `TermMode::default()` enables ALTERNATE_SCROLL,
    // so a fresh terminal is NOT `VtModes::default()`.
    fn baseline() -> VtModes {
        VtModes {
            alternate_scroll: true,
            ..VtModes::default()
        }
    }

    #[test]
    fn fresh_terminal_reports_alacritty_baseline() {
        let vt = AlacrittyVt::new(80, 24);
        assert_eq!(vt.modes(), baseline());
    }

    #[test]
    fn decset_sets_flags_and_enums() {
        let vt = vt_after(b"\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            vt.modes(),
            VtModes {
                app_cursor: true,
                bracketed_paste: true,
                focus_in_out: true,
                mouse_tracking: MouseTracking::Clicks,
                mouse_encoding: MouseEncoding::Sgr,
                ..baseline()
            }
        );
    }

    #[test]
    fn mouse_encodings_are_exclusive() {
        let vt = vt_after(b"\x1b[?1005h\x1b[?1006h");
        assert_eq!(vt.modes().mouse_encoding, MouseEncoding::Sgr);
        let vt = vt_after(b"\x1b[?1006h\x1b[?1005h");
        assert_eq!(vt.modes().mouse_encoding, MouseEncoding::Utf8);
    }

    #[test]
    fn mouse_tracking_levels_replace_each_other() {
        let vt = vt_after(b"\x1b[?1000h\x1b[?1003h");
        assert_eq!(vt.modes().mouse_tracking, MouseTracking::Motion);
        let vt = vt_after(b"\x1b[?1002h");
        assert_eq!(vt.modes().mouse_tracking, MouseTracking::Drag);
    }

    #[test]
    fn alt_screen_and_decrst_roundtrip() {
        let vt = vt_after(b"\x1b[?1049h");
        assert!(vt.modes().alt_screen);
        let vt = vt_after(b"\x1b[?1006h\x1b[?1006l");
        assert_eq!(vt.modes().mouse_encoding, MouseEncoding::X10);
        let vt = vt_after(b"\x1b[?1007l");
        assert!(!vt.modes().alternate_scroll);
    }

    const VIEWPORT_FILL_ROWS: usize = 23;
    const SEEDED_HISTORY_ROWS: usize = 10;

    // NOTE: alacritty pushes a row into history only once the cursor already
    // sits on the last screen line, so the first `VIEWPORT_FILL_ROWS` newlines
    // of a 24-row grid fill the viewport without growing `history_size`. The
    // precondition assert keeps a change in that accounting from silently
    // collapsing every `display_offset` expectation below to zero.
    fn vt_with_history(history_rows: usize) -> AlacrittyVt {
        let bytes: Vec<u8> = (0..history_rows + VIEWPORT_FILL_ROWS)
            .flat_map(|i| format!("l{i}\r\n").into_bytes())
            .collect();
        let vt = vt_after(&bytes);
        assert_eq!(vt.term.grid().history_size(), history_rows);
        vt
    }

    #[test]
    fn positive_delta_scrolls_into_history() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        assert_eq!(vt.display_offset(), 0);
        vt.scroll(3);
        assert_eq!(vt.display_offset(), 3);
        vt.scroll(4);
        assert_eq!(vt.display_offset(), 7);
    }

    #[test]
    fn negative_delta_scrolls_toward_the_live_tail() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        vt.scroll(7);
        vt.scroll(-4);
        assert_eq!(vt.display_offset(), 3);
        vt.scroll(-3);
        assert_eq!(vt.display_offset(), 0);
    }

    #[test]
    fn scroll_by_zero_leaves_the_viewport_untouched() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        vt.scroll(0);
        assert_eq!(vt.display_offset(), 0);
        vt.scroll(4);
        vt.scroll(0);
        assert_eq!(vt.display_offset(), 4);
    }

    // NOTE: the clamp bound must stay finite. `Grid::scroll_display` adds
    // `delta` to `display_offset` with a plain `i32` add, so `i32::MAX` here
    // would overflow and panic under the overflow checks enabled in dev/test.
    #[test]
    fn scroll_clamps_at_the_top_of_history() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        vt.scroll(SEEDED_HISTORY_ROWS as i32 + 100);
        assert_eq!(vt.display_offset(), SEEDED_HISTORY_ROWS as u32);
        vt.scroll(1);
        assert_eq!(vt.display_offset(), SEEDED_HISTORY_ROWS as u32);
    }

    #[test]
    fn scroll_clamps_at_the_live_tail() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        vt.scroll(5);
        vt.scroll(-1000);
        assert_eq!(vt.display_offset(), 0);
        vt.scroll(-1000);
        assert_eq!(vt.display_offset(), 0);
    }

    #[test]
    fn scroll_without_scrollback_is_a_noop() {
        let mut vt = vt_after(b"one\r\ntwo\r\nthree");
        vt.scroll(5);
        assert_eq!(vt.display_offset(), 0);
        let mut vt = vt_with_history(0);
        vt.scroll(5);
        assert_eq!(vt.display_offset(), 0);
    }

    // NOTE: the history must be seeded on the primary screen before switching,
    // otherwise this passes for the trivial reason that nothing was scrollable
    // in the first place. The alternate grid is built with zero scrollback
    // capacity, so it has nowhere to scroll to.
    #[test]
    fn scroll_on_the_alternate_screen_is_a_noop() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        vt.interpret(b"\x1b[?1049h");
        assert!(vt.modes().alt_screen);
        vt.scroll(5);
        assert_eq!(vt.display_offset(), 0);
    }

    #[test]
    fn at_scroll_bottom_tracks_the_viewport() {
        let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
        assert!(vt.at_scroll_bottom());
        vt.scroll(3);
        assert!(!vt.at_scroll_bottom());
        vt.scroll(-3);
        assert!(vt.at_scroll_bottom());
    }

    /// Row count of the grid every fixture in this module builds.
    const GRID_ROWS: u16 = 24;

    // NOTE: a fresh `Term` starts fully damaged for the bootstrap paint, so a
    // test that wants to observe only what its own bytes staged must clear
    // both halves — the staged value AND alacritty's accumulator. Stands in
    // for `frames()`, which is still `todo!()`.
    fn drain_staged(vt: &mut AlacrittyVt) {
        vt.pending_damage = None;
        vt.term.reset_damage();
    }

    #[test]
    fn empty_chunk_is_not_a_cycle() {
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        assert_eq!(vt.interpret(b""), None);
    }

    #[test]
    fn empty_chunk_leaves_staged_damage_untouched() {
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        drain_staged(&mut vt);
        vt.interpret(b"hi");
        let staged = vt.pending_damage.clone();
        assert!(
            staged.is_some(),
            "precondition: interpreting a non-empty chunk must stage damage"
        );
        assert_eq!(vt.interpret(b""), None);
        assert_eq!(
            vt.pending_damage, staged,
            "an empty chunk must neither re-read the damage tracker nor clear the staged value"
        );
    }

    #[test]
    fn the_first_interpret_on_a_fresh_vt_reports_full() {
        // Whatever the chunk contains: the bootstrap `Full` outranks it.
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        assert_eq!(vt.interpret(b"x"), Some(DamageVerdict::Full));
    }

    #[test]
    fn a_single_row_write_classifies_as_at_most_one_row() {
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        drain_staged(&mut vt);
        assert_eq!(vt.interpret(b"hi"), Some(DamageVerdict::AtMostOneRow));
    }

    #[test]
    fn a_multi_row_write_classifies_as_many_rows() {
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        drain_staged(&mut vt);
        assert_eq!(
            vt.interpret(b"one\r\ntwo\r\nthree"),
            Some(DamageVerdict::ManyRows { rows: 3 })
        );
    }

    #[test]
    fn insert_mode_classifies_as_full() {
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        drain_staged(&mut vt);
        assert_eq!(vt.interpret(b"\x1b[4h"), Some(DamageVerdict::Full));
    }

    #[test]
    fn interpret_stages_the_damage_it_classified() {
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        drain_staged(&mut vt);
        assert_eq!(
            vt.interpret(b"one\r\ntwo\r\nthree"),
            Some(DamageVerdict::ManyRows { rows: 3 })
        );
        assert_eq!(
            vt.pending_damage,
            Some(DirtyRows::Rows(vec![0, 1, 2])),
            "the staged rows must be the ones the verdict was computed from"
        );
    }

    #[test]
    fn staged_damage_accumulates_across_chunks() {
        // The reason staging is a plain replacement and not a union:
        // alacritty keeps expanding `damage.lines` until `reset_damage`, so
        // the newest read already covers every chunk since the last emit.
        let mut vt = AlacrittyVt::new(80, GRID_ROWS);
        drain_staged(&mut vt);
        vt.interpret(b"a");
        vt.interpret(b"\r\n\r\nb");
        let Some(DirtyRows::Rows(rows)) = &vt.pending_damage else {
            panic!(
                "expected staged partial damage, got {:?}",
                vt.pending_damage
            );
        };
        assert!(
            rows.contains(&0),
            "the row written by the first chunk must survive into the second cycle, got {rows:?}"
        );
        assert!(
            rows.contains(&2),
            "the row written by the second chunk must be staged, got {rows:?}"
        );
    }

    #[test]
    fn a_fresh_vt_stages_bootstrap_full_damage() {
        // Without this, a `frames()` call that precedes the first `interpret`
        // finds nothing staged and the bootstrap paint never reaches the
        // renderer.
        assert_eq!(
            AlacrittyVt::new(80, GRID_ROWS).pending_damage,
            Some(DirtyRows::Full)
        );
    }

    // NOTE: `TermDamageIterator::new` truncates the trailing `display_offset`
    //       entries BEFORE filtering (alacritty `term/mod.rs:194-198`). Once
    //       `display_offset >= screen_lines` the whole slice is gone, so the
    //       iterator yields nothing even though `Term::damage` always damages
    //       the cursor — which is what makes `DamageVerdict::Idle` reachable.
    //       alacritty's own `damage_public_usage` (`term/mod.rs:3025-3036`)
    //       asserts the same empty `Partial`.
    #[test]
    fn a_viewport_fully_in_scrollback_stages_empty_damage() {
        let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
        vt.scroll(i32::from(GRID_ROWS));
        assert_eq!(
            vt.display_offset(),
            u32::from(GRID_ROWS),
            "precondition: the viewport must sit entirely in scrollback"
        );
        drain_staged(&mut vt);
        assert_eq!(vt.interpret(b"\x1b[H"), Some(DamageVerdict::Idle));
        assert_eq!(vt.pending_damage, Some(DirtyRows::Rows(Vec::new())));
    }
}
