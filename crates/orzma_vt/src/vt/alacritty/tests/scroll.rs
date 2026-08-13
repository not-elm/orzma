//! Scroll, display-offset, live-tail, and scroll-damage tests.

use super::*;

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
