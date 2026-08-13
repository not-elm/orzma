//! Tests for [`AlacrittyVtBackend`].

use super::*;
use crate::schema::{CellSide, SelectionGeometry};
use alacritty_terminal::index::Point as AlacPoint;

fn vt_after(bytes: &[u8]) -> AlacrittyVtBackend {
    let mut vt = AlacrittyVtBackend::new(80, 24);
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
    let vt = AlacrittyVtBackend::new(80, 24);
    assert_eq!(vt.modes(), baseline());
}

/// Asserts that `resize` reshapes the emulated grid to the
/// requested dimensions.
///
/// Case: the window-resize path — `OrzmaTerm::resize` delegates
/// here after the PTY ioctl. The non-square target catches a
/// cols/rows transposition into `LocalDim`, which would reflow
/// every line at the wrong width while the child renders at the
/// correct one.
#[test]
fn resize_updates_the_grid_size() {
    let mut vt = AlacrittyVtBackend::new(80, 24);
    vt.resize(120, 40);
    assert_eq!(
        vt.grid_size(),
        GridSize {
            cols: 120,
            rows: 40
        }
    );
}

/// Asserts that `resize` stages full damage.
///
/// Case: a resize reflows the whole grid, but no PTY output need
/// follow — an idle shell prompt stays idle. Without staged `Full`
/// damage the next `frames()` call finds nothing to emit and the
/// renderer keeps drawing the old grid until unrelated output
/// arrives (the trait doc pins this repaint contract).
#[test]
fn resize_stages_full_damage() {
    let mut vt = AlacrittyVtBackend::new(80, 24);
    drain_staged(&mut vt);
    vt.resize(120, 40);
    assert_eq!(vt.pending_damage, Some(Damage::Full));
}

/// Asserts that `grid_size` maps the term's columns to `cols` and its
/// screen lines to `rows`.
///
/// Case: paging on a non-square 80x24 grid, where half a page must
/// resolve from the 24-row axis rather than the 80-column one.
#[test]
fn grid_size_maps_cols_and_rows_from_the_term() {
    assert_eq!(
        AlacrittyVtBackend::new(80, 24).grid_size(),
        GridSize { cols: 80, rows: 24 }
    );
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
fn vt_with_history(history_rows: usize) -> AlacrittyVtBackend {
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
    assert_eq!(vt.display_offset(), DisplayOffset(0));
    vt.scroll(Scroll::Delta(3));
    assert_eq!(vt.display_offset(), DisplayOffset(3));
    vt.scroll(Scroll::Delta(4));
    assert_eq!(vt.display_offset(), DisplayOffset(7));
}

#[test]
fn negative_delta_scrolls_toward_the_live_tail() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(7));
    vt.scroll(Scroll::Delta(-4));
    assert_eq!(vt.display_offset(), DisplayOffset(3));
    vt.scroll(Scroll::Delta(-3));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
}

#[test]
fn scroll_by_zero_leaves_the_viewport_untouched() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(0));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
    vt.scroll(Scroll::Delta(4));
    vt.scroll(Scroll::Delta(0));
    assert_eq!(vt.display_offset(), DisplayOffset(4));
}

// NOTE: the clamp bound must stay finite. `Grid::scroll_display` adds
// `delta` to `display_offset` with a plain `i32` add, so `i32::MAX` here
// would overflow and panic under the overflow checks enabled in dev/test.
#[test]
fn scroll_clamps_at_the_top_of_history() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(SEEDED_HISTORY_ROWS as i32 + 100));
    assert_eq!(
        vt.display_offset(),
        DisplayOffset(SEEDED_HISTORY_ROWS as u32)
    );
    vt.scroll(Scroll::Delta(1));
    assert_eq!(
        vt.display_offset(),
        DisplayOffset(SEEDED_HISTORY_ROWS as u32)
    );
}

#[test]
fn scroll_clamps_at_the_live_tail() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(5));
    vt.scroll(Scroll::Delta(-1000));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
    vt.scroll(Scroll::Delta(-1000));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
}

#[test]
fn scroll_without_scrollback_is_a_noop() {
    let mut vt = vt_after(b"one\r\ntwo\r\nthree");
    vt.scroll(Scroll::Delta(5));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
    let mut vt = vt_with_history(0);
    vt.scroll(Scroll::Delta(5));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
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
    vt.scroll(Scroll::Delta(5));
    assert_eq!(vt.display_offset(), DisplayOffset(0));
}

/// Asserts that the live-tail predicate follows the viewport in both
/// directions, not just away from the tail.
///
/// Case: the scroll-on-input policy gates on this predicate, so a
/// value that latches `false` after a scroll back down would make
/// every later keystroke re-snap a viewport that never moved, and
/// stage damage for a repaint nothing asked for.
#[test]
fn is_at_live_tail_tracks_the_viewport() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    assert!(vt.is_at_live_tail());
    vt.scroll(Scroll::Delta(3));
    assert!(!vt.is_at_live_tail());
    vt.scroll(Scroll::Delta(-3));
    assert!(vt.is_at_live_tail());
}

/// Asserts that every absolute and paged `Scroll` variant moves the
/// viewport in its own direction and magnitude, with a half page
/// being `screen_lines / 2` rows.
///
/// Case: `Scroll::to_alacritty_scroll` is a seven-arm match onto a
/// smaller enum — a transposed arm (PageUp↔PageDown, Top↔Bottom,
/// HalfPageUp↔HalfPageDown) compiles cleanly and inverts the
/// motion, and the `Delta` tests above cannot see it. The history
/// is deeper than one screen so `PageUp` lands on the page size,
/// not the clamp.
#[test]
fn absolute_and_paged_scrolls_map_to_their_directions() {
    let history = usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS;
    let mut vt = vt_with_history(history);
    vt.scroll(Scroll::PageUp);
    assert_eq!(vt.display_offset(), DisplayOffset(u32::from(GRID_ROWS)));
    vt.scroll(Scroll::PageDown);
    assert_eq!(vt.display_offset(), DisplayOffset(0));
    vt.scroll(Scroll::HalfPageUp);
    assert_eq!(vt.display_offset(), DisplayOffset(u32::from(GRID_ROWS / 2)));
    vt.scroll(Scroll::HalfPageDown);
    assert_eq!(vt.display_offset(), DisplayOffset(0));
    vt.scroll(Scroll::Top);
    assert_eq!(vt.display_offset(), DisplayOffset(history as u32));
    vt.scroll(Scroll::Bottom);
    assert_eq!(vt.display_offset(), DisplayOffset(0));
}

/// Asserts that a scroll which moved the viewport stages full
/// damage.
///
/// Case: the trait doc's repaint contract. A scroll changes every
/// visible row but produces no PTY output; without staged `Full`
/// damage the next `frames()` call finds nothing to emit, and an
/// armed coalescer fires an emit for a repaint that never comes.
#[test]
fn scroll_stages_full_damage_when_the_viewport_moves() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    drain_staged(&mut vt);
    vt.scroll(Scroll::Delta(3));
    assert_eq!(vt.pending_damage, Some(Damage::Full));
}

/// Asserts that a scroll which did not move the viewport leaves the
/// staged damage exactly as it was.
///
/// Case: the trait doc's "a no-op call stages no damage" invariant,
/// pinned against the destructive failure mode — an implementation
/// ending in `else { pending_damage = None }` passes a clean-state
/// check while silently discarding earlier un-emitted output (same
/// shape as `empty_chunk_leaves_staged_damage_untouched`).
#[test]
fn a_no_op_scroll_preserves_staged_damage() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    drain_staged(&mut vt);
    // NOTE: seeded directly rather than via `interpret` — chunk
    // damage staging is itself still unimplemented (the four
    // pre-existing damage-test failures), and this test must not
    // depend on that gap.
    vt.pending_damage = Some(Damage::Delta(vec![0].into()));
    let staged = vt.pending_damage.clone();
    vt.scroll(Scroll::Delta(0));
    vt.scroll(Scroll::Bottom);
    assert_eq!(vt.pending_damage, staged);
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    drain_staged(&mut vt);
    vt.scroll(Scroll::Delta(0));
    assert_eq!(vt.pending_damage, None, "clean state stays clean");
}

/// Asserts that scrolling on the alternate screen stages no damage.
///
/// Case: the alternate grid has no scrollback, so every scroll
/// there is a no-op — but entering the alternate screen stages its
/// own damage, which must be drained first or it masks a violation
/// of the no-op invariant on this backend.
#[test]
fn scrolling_the_alternate_screen_stages_no_damage() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.interpret(b"\x1b[?1049h");
    assert!(vt.modes().alt_screen, "precondition: alt screen entered");
    drain_staged(&mut vt);
    vt.scroll(Scroll::Delta(5));
    assert_eq!(vt.pending_damage, None);
}

/// Row count of the grid every fixture in this module builds.
const GRID_ROWS: u16 = 24;

// NOTE: a fresh `Term` starts fully damaged for the bootstrap paint, so a
// test that wants to observe only what its own bytes staged must clear
// both halves — the staged value AND alacritty's accumulator. Stands in
// for `frames()`, which is still `todo!()`.
fn drain_staged(vt: &mut AlacrittyVtBackend) {
    vt.pending_damage = None;
    vt.term.reset_damage();
}

#[test]
fn empty_chunk_is_not_a_cycle() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    assert_eq!(vt.interpret(b""), None);
}

#[test]
fn empty_chunk_leaves_staged_damage_untouched() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
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
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    assert_eq!(vt.interpret(b"x"), Some(Damage::Full));
}

#[test]
fn a_single_row_write_reports_one_dirty_row() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(vt.interpret(b"hi"), Some(Damage::Delta(vec![0].into())));
}

#[test]
fn a_multi_row_write_reports_each_dirty_row() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(
        vt.interpret(b"one\r\ntwo\r\nthree"),
        Some(Damage::Delta(vec![0, 1, 2].into()))
    );
}

#[test]
fn insert_mode_reports_full_damage() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(vt.interpret(b"\x1b[4h"), Some(Damage::Full));
}

#[test]
fn interpret_stages_the_damage_it_returns() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(
        vt.interpret(b"one\r\ntwo\r\nthree"),
        Some(Damage::Delta(vec![0, 1, 2].into()))
    );
    assert_eq!(
        vt.pending_damage,
        Some(Damage::Delta(vec![0, 1, 2].into())),
        "the staged rows must be the ones the call returned"
    );
}

/// Asserts that a row damaged by an earlier chunk survives into the
/// staged value of a later one.
///
/// Case: two PTY chunks arrive between emits. This holds for two
/// independent reasons — alacritty keeps expanding `damage.lines`
/// until `reset_damage`, and staging merges — so it passes either
/// way and does not guard the merge on its own.
#[test]
fn staged_damage_accumulates_across_chunks() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    vt.interpret(b"a");
    vt.interpret(b"\r\n\r\nb");
    let Some(Damage::Delta(rows)) = &vt.pending_damage else {
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
        AlacrittyVtBackend::new(80, GRID_ROWS).pending_damage,
        Some(Damage::Full)
    );
}

// NOTE: `TermDamageIterator::new` truncates the trailing `display_offset`
//       entries BEFORE filtering (alacritty `term/mod.rs:194-198`). Once
//       `display_offset >= screen_lines` the whole slice is gone, so the
//       iterator yields nothing even though `Term::damage` always damages
//       the cursor — which is what makes an empty `Delta` reachable.
//       alacritty's own `damage_public_usage` (`term/mod.rs:3025-3036`)
//       asserts the same empty `Partial`.
#[test]
fn a_viewport_fully_in_scrollback_stages_empty_damage() {
    let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(i32::from(GRID_ROWS)));
    assert_eq!(
        vt.display_offset(),
        DisplayOffset(u32::from(GRID_ROWS)),
        "precondition: the viewport must sit entirely in scrollback"
    );
    drain_staged(&mut vt);
    assert_eq!(
        vt.interpret(b"\x1b[H"),
        Some(Damage::Delta(DamageRows::default()))
    );
    assert_eq!(
        vt.pending_damage,
        Some(Damage::Delta(DamageRows::default()))
    );
}

fn cell(x: u16, y: i16) -> ViewportPoint {
    ViewportPoint { row: y, column: x }
}

fn start_simple(vt: &mut AlacrittyVtBackend, x: u16, y: i16) {
    vt.apply_selection(SelectionOp::StartAt {
        cell: cell(x, y),
        side: CellSide::Left,
        kind: SelectionKind::Simple,
    })
    .unwrap();
}

fn update_to(vt: &mut AlacrittyVtBackend, x: u16, y: i16, side: CellSide) {
    vt.apply_selection(SelectionOp::UpdateTo {
        cell: cell(x, y),
        side,
    })
    .unwrap();
}

fn enter_vi_at(vt: &mut AlacrittyVtBackend, x: usize, y: i32) {
    vt.term.toggle_vi_mode();
    vt.term.vi_mode_cursor.point = AlacPoint::new(Line(y), Column(x));
}

/// Asserts that a one-cell `StartAt` yields a renderable, non-empty
/// selection with `Linear` geometry.
///
/// Case: a mouse press followed by copy. A bare `Selection::new` is
/// empty when both ends coincide, so the implementation must apply
/// the opposite-side update recipe or a click-then-copy yields
/// nothing. Also pins the Simple → Linear geometry arm.
#[test]
fn start_at_anchors_a_non_empty_selection() {
    let mut vt = vt_after(b"hi");
    start_simple(&mut vt, 0, 0);
    let range = vt.selection_range().expect("one-cell start must render");
    assert_eq!(range.start, ViewportPoint { row: 0, column: 0 });
    assert_eq!(range.end, ViewportPoint { row: 0, column: 0 });
    assert_eq!(range.geometry, SelectionGeometry::Linear);
    assert_eq!(vt.selected_text().as_deref(), Some("h"));
}

/// Asserts that `UpdateTo` moves only the moving end; the anchor
/// stays where `StartAt` put it.
///
/// Case: the basic drag — press on the first cell, drag right across
/// four more. The extracted text must cover the whole span.
#[test]
fn update_to_extends_the_moving_end() {
    let mut vt = vt_after(b"abcdefghij");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 4, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("abcde"));
}

/// Asserts that `UpdateTo` with no active selection changes nothing.
///
/// Case: alacritty wipes the selection on an alt-screen swap while
/// the input glue may still deliver one more drag event; the stray
/// update must neither panic nor conjure a selection.
#[test]
fn update_to_without_a_selection_is_a_no_op() {
    let mut vt = vt_after(b"abc");
    update_to(&mut vt, 2, 0, CellSide::Right);
    assert_eq!(vt.selection_range(), None);
}

/// Asserts that `StartAt` resolves viewport rows through the display
/// offset onto the scrollback rows the user actually sees.
///
/// Case: selecting while scrolled back. The `y - display_offset`
/// translation is this crate's code; getting it wrong selects a live
/// row hidden below the viewport instead of the visible history row.
#[test]
fn start_at_translates_viewport_rows_through_display_offset() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(3));
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 1, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("l7"));
}

/// Asserts that the end-cell side decides whether the cell under the
/// pointer is included.
///
/// Case: the CellSide → alacritty `Side` mapping is a two-arm match;
/// a transposition compiles cleanly and off-by-ones every selection
/// the user ever drags.
#[test]
fn cell_side_decides_inclusion_of_the_boundary_cells() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 3, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("abcd"));
    vt.apply_selection(SelectionOp::Clear).unwrap();
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 3, 0, CellSide::Left);
    assert_eq!(vt.selected_text().as_deref(), Some("abc"));
}

/// Asserts that a drag toward the top-left reports a normalized
/// range with `start` at the top.
///
/// Case: an upward drag. `SelectionRange`'s doc pins start as the
/// top-left of the selected cells; a renderer given anchor-order
/// endpoints would rasterize a negative-height span.
#[test]
fn a_backward_drag_normalizes_start_before_end() {
    let mut vt = vt_after(b"one\r\ntwo\r\nthree");
    start_simple(&mut vt, 5, 2);
    update_to(&mut vt, 1, 0, CellSide::Left);
    let range = vt.selection_range().expect("backward drag must render");
    assert_eq!(range.start.row, 0);
    assert_eq!(range.end.row, 2);
}

/// Asserts that `StartAtViCursor` anchors at the vi cursor the VT
/// tracks internally.
///
/// Case: the vi-mode `v` press. The host deliberately sends no cell
/// because it does not track the vi cursor; the VT must resolve the
/// anchor itself.
#[test]
fn start_at_vi_cursor_anchors_at_the_vi_cursor() {
    let mut vt = vt_after(b"hello");
    enter_vi_at(&mut vt, 2, 0);
    vt.apply_selection(SelectionOp::StartAtViCursor {
        kind: SelectionKind::Simple,
    })
    .unwrap();
    assert_eq!(vt.selected_text().as_deref(), Some("l"));
}

/// Asserts that `ChangeKind` switches granularity while preserving
/// the original anchor, spanning to the current vi cursor.
///
/// Case: copy-mode `v` then `V` without leaving vi mode (the old
/// handle.rs anchor-preservation contract). Re-anchoring at the vi
/// cursor instead would collapse the selection the user built up.
#[test]
fn change_kind_switches_granularity_and_keeps_the_anchor() {
    let mut vt = vt_after(b"abcdefghij\r\nklmnopqrst");
    enter_vi_at(&mut vt, 2, 0);
    vt.apply_selection(SelectionOp::StartAtViCursor {
        kind: SelectionKind::Simple,
    })
    .unwrap();
    vt.term.vi_mode_cursor.point = AlacPoint::new(Line(1), Column(7));
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(
        vt.selected_text().as_deref(),
        Some("abcdefghij\nklmnopqrst\n"),
        "Lines spanning the preserved row-0 anchor through the vi cursor on row 1"
    );
}

/// Asserts that `ChangeKind` with no active selection changes
/// nothing.
///
/// Case: the host resolves the v/V toggle by reading the current
/// kind, but the selection can vanish between that read and the
/// apply; `ChangeKind` must not conjure a selection from nothing.
#[test]
fn change_kind_without_a_selection_is_a_no_op() {
    let mut vt = vt_after(b"abc");
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_range(), None);
}

/// Asserts that `Clear` drops the selection AND the stored anchor a
/// later `ChangeKind` would rebuild from.
///
/// Case: clear, then press `V`. An implementation keeping the saved
/// anchor would resurrect a zombie selection from pre-clear state
/// instead of treating the change as a no-op.
#[test]
fn clear_discards_the_selection_and_the_stored_anchor() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.selection_range(), None);
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_range(), None, "no zombie from a stale anchor");
}

/// Asserts that an alt-screen swap invalidates the selection, its
/// reported kind, and the stored anchor.
///
/// Case: opening vim mid-selection. alacritty clears only its own
/// `Term::selection`; the backend's separately stored anchor must
/// not let `ChangeKind` rebuild from stale primary-screen state, and
/// the getters must not serve cached values.
#[test]
fn an_alt_screen_swap_invalidates_selection_and_anchor() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    vt.interpret(b"\x1b[?1049h");
    assert!(vt.modes().alt_screen, "precondition: alt screen entered");
    assert_eq!(vt.selection_range(), None);
    assert_eq!(vt.selection_kind(), None);
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_range(), None, "no rebuild from a stale anchor");
}

/// Asserts that a `Lines` start selects the logical row: full-width
/// range, `Lines` geometry, and content-plus-newline text.
///
/// Case: vi `V` (or a triple click). The text is the populated row
/// plus a trailing `\n` — not an 80-column space-padded row — per
/// the logical-line extraction the old renderer relied on. Also pins
/// the Lines → Lines geometry arm.
#[test]
fn lines_kind_selects_the_logical_row_with_trailing_newline() {
    let mut vt = vt_after(b"hello world");
    vt.apply_selection(SelectionOp::StartAt {
        cell: cell(3, 0),
        side: CellSide::Left,
        kind: SelectionKind::Lines,
    })
    .unwrap();
    let range = vt.selection_range().expect("Lines start must render");
    assert_eq!(range.start, ViewportPoint { row: 0, column: 0 });
    assert_eq!(range.end, ViewportPoint { row: 0, column: 79 });
    assert_eq!(range.geometry, SelectionGeometry::Lines);
    assert_eq!(vt.selected_text().as_deref(), Some("hello world\n"));
}

/// Asserts that dragging back onto the anchor cell/side makes the
/// selection empty for `selection_range` and `selected_text` while
/// `selection_kind` still reports the live selection object.
///
/// Case: a drag that returns to its starting point. The trait doc
/// says range/text are `None` for an empty selection but kind is
/// `None` only when NO selection exists — this pins the three
/// getters to one consistent notion of "empty".
#[test]
fn an_update_back_onto_the_anchor_empties_the_selection() {
    let mut vt = vt_after(b"abc");
    start_simple(&mut vt, 1, 0);
    update_to(&mut vt, 1, 0, CellSide::Left);
    assert_eq!(vt.selection_range(), None);
    assert_eq!(vt.selected_text(), None);
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Simple));
}

/// Asserts that every operation which changes the visible selection
/// stages full damage: StartAt, a moving UpdateTo, ChangeKind, and
/// Clear.
///
/// Case: the trait's repaint contract. alacritty excludes Selection
/// from `Term::damage()`, so an implementation staging damage only
/// for StartAt/Clear would leave drags and v/V switches visually
/// stale while passing every state test.
#[test]
fn every_visible_selection_change_stages_full_damage() {
    let mut vt = vt_after(b"abcdefghij");
    drain_staged(&mut vt);
    start_simple(&mut vt, 0, 0);
    assert_eq!(vt.pending_damage, Some(Damage::Full), "StartAt");
    drain_staged(&mut vt);
    update_to(&mut vt, 4, 0, CellSide::Right);
    assert_eq!(vt.pending_damage, Some(Damage::Full), "UpdateTo");
    drain_staged(&mut vt);
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.pending_damage, Some(Damage::Full), "ChangeKind");
    drain_staged(&mut vt);
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.pending_damage, Some(Damage::Full), "Clear");
}

/// Asserts that no-op selection operations leave the staged damage
/// exactly as it was.
///
/// Case: the "a no-op stages no damage" invariant, pinned against
/// the destructive failure mode — an implementation ending in
/// `else { pending_damage = None }` passes a clean-state check while
/// silently discarding earlier un-emitted output (same shape as the
/// scroll no-op test).
#[test]
fn no_op_selection_ops_preserve_staged_damage() {
    let mut vt = vt_after(b"abc");
    drain_staged(&mut vt);
    vt.pending_damage = Some(Damage::Delta(vec![0].into()));
    let staged = vt.pending_damage.clone();
    update_to(&mut vt, 2, 0, CellSide::Right);
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.pending_damage, staged);
    let mut vt = vt_after(b"abc");
    drain_staged(&mut vt);
    update_to(&mut vt, 2, 0, CellSide::Right);
    assert_eq!(vt.pending_damage, None, "clean state stays clean");
}

/// Asserts that a fresh VT reports no selection range.
///
/// Case: the renderer treats `None` as "no overlay"; a non-`None`
/// default would paint a phantom selection on boot.
#[test]
fn selection_range_is_none_on_a_fresh_vt() {
    assert_eq!(
        AlacrittyVtBackend::new(80, GRID_ROWS).selection_range(),
        None
    );
}

/// Asserts that display scrolling shifts the projected viewport rows
/// and clamps off-viewport endpoints to the -1 / row-count
/// sentinels.
///
/// Case: select, then scroll. The selection is pinned to grid
/// content, so its viewport projection moves opposite the scroll and
/// partially visible selections need the sentinel rows for the
/// renderer to draw the on-screen part.
#[test]
fn display_scroll_shifts_and_clamps_the_projected_range() {
    let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 5, 15, CellSide::Right);
    vt.scroll(Scroll::Delta(3));
    let range = vt.selection_range().expect("selection survives scroll");
    assert_eq!((range.start.row, range.end.row), (3, 18), "pure shift");
    vt.scroll(Scroll::Delta(7));
    let range = vt.selection_range().expect("still partially visible");
    assert_eq!(range.start.row, 10);
    assert_eq!(range.end.row, GRID_ROWS as i16, "below-viewport sentinel");
    vt.apply_selection(SelectionOp::Clear).unwrap();
    start_simple(&mut vt, 0, 5);
    update_to(&mut vt, 5, 20, CellSide::Right);
    vt.scroll(Scroll::Bottom);
    let range = vt.selection_range().expect("still partially visible");
    assert_eq!(range.start.row, -1, "above-viewport sentinel");
    assert_eq!(range.end.row, 10);
}

/// Asserts that `selection_kind` tracks the active granularity and
/// resets on `Clear`.
///
/// Case: the vi v/V toggle predicate reads this to choose between
/// clearing (same kind), switching (different kind), and starting
/// (none); a stale kind flips that decision.
#[test]
fn selection_kind_reports_the_active_granularity_and_resets() {
    let mut vt = vt_after(b"abcdef");
    assert_eq!(vt.selection_kind(), None);
    start_simple(&mut vt, 0, 0);
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Simple));
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Lines));
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.selection_kind(), None);
}

/// Asserts that `selected_text` is `None` on a fresh VT and after a
/// `Clear`.
///
/// Case: the copy path treats `None` as "nothing to copy". A cached
/// or `Some("")` result would overwrite the user's clipboard with an
/// empty string.
#[test]
fn selected_text_is_none_without_a_selection() {
    let mut vt = vt_after(b"abc");
    assert_eq!(vt.selected_text(), None);
    start_simple(&mut vt, 0, 0);
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.selected_text(), None);
}

/// Asserts that `selected_text` extracts wide characters exactly
/// once, joins soft-wrapped rows without a newline, and keeps hard
/// line breaks.
///
/// Case: copying CJK output and long wrapped lines. Wide-char spacer
/// cells are the classic double-or-drop bug, and a soft wrap that
/// leaks a `\n` corrupts every pasted long command line.
#[test]
fn selected_text_handles_wide_and_wrapped_content() {
    let mut vt = vt_after("あ".as_bytes());
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 1, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("あ"));

    let long = "x".repeat(85);
    let mut vt = vt_after(long.as_bytes());
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 4, 1, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some(long.as_str()));

    let mut vt = vt_after(b"ab\r\ncd");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 1, 1, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("ab\ncd"));
}
