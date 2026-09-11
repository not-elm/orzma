//! Tests for viewport motion over the scrollback.

use super::*;

/// Builds a four-row screen with `rows` rows of scrollback behind it,
/// which puts the page size at 4 and the half page at 2.
fn scrolled_screen(rows: usize) -> Screen {
    let mut screen = tall_screen();
    for _ in 0..rows {
        screen.state.line = ScreenLine(3);
        screen.line_feed();
    }
    assert_eq!(screen.grid.history_len(), rows);
    screen
}

/// Asserts that a delta moves the viewport by its signed count,
/// positive toward older output and negative toward the live tail.
///
/// Case: the user rolls the mouse wheel back three notches and then
/// forward two.
#[test]
fn a_delta_moves_the_viewport_by_its_signed_count() {
    let mut screen = scrolled_screen(5);
    assert_eq!(screen.scroll(Scroll::Delta(3)), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(3));
    assert_eq!(screen.scroll(Scroll::Delta(-2)), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}

/// Asserts that a page motion moves a whole screenful, with no row
/// carried over between the old view and the new one.
///
/// Case: the user presses Shift+PageUp and then Shift+PageDown.
#[test]
fn a_page_moves_one_whole_screenful() {
    let mut screen = scrolled_screen(10);
    assert_eq!(screen.scroll(Scroll::PageUp), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(4));
    assert_eq!(screen.scroll(Scroll::PageDown), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a half-page motion moves half a screenful.
///
/// Case: the user presses the vi-mode `Ctrl-U` and `Ctrl-D` motions.
#[test]
fn a_half_page_moves_half_a_screenful() {
    let mut screen = scrolled_screen(10);
    assert_eq!(screen.scroll(Scroll::HalfPageUp), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(2));
    assert_eq!(screen.scroll(Scroll::HalfPageDown), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that `Top` lands on the oldest retained line and `Bottom`
/// returns to the live tail.
///
/// Case: the user jumps to the start of the scrollback and back with
/// the vi-mode `gg` and `G` motions.
#[test]
fn top_and_bottom_jump_to_the_extremes() {
    let mut screen = scrolled_screen(7);
    assert_eq!(screen.scroll(Scroll::Top), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(7));
    assert_eq!(screen.scroll(Scroll::Bottom), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a motion reaching past the oldest retained line stops
/// on it rather than running off the end of the history.
///
/// Case: the user spins the wheel hard while only a few lines of
/// scrollback exist.
#[test]
fn a_motion_past_the_oldest_line_clamps_to_it() {
    let mut screen = scrolled_screen(2);
    assert_eq!(screen.scroll(Scroll::Delta(100)), Some(DamageSpan::Full));
    assert_eq!(screen.display_offset(), DisplayOffset(2));
}

/// Asserts that a motion reaching past the live tail stops on it,
/// including the most negative delta representable.
///
/// Case: a host that computes its wheel delta from a large pixel
/// scroll sends one far bigger than the viewport can absorb.
#[test]
fn a_motion_past_the_live_tail_clamps_to_it() {
    let mut screen = scrolled_screen(5);
    screen.scroll(Scroll::Delta(3));
    assert_eq!(
        screen.scroll(Scroll::Delta(i32::MIN)),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a zero delta reports no motion.
///
/// Case: a trackpad gesture rounds to zero lines between frames.
#[test]
fn a_zero_delta_reports_no_motion() {
    let mut screen = scrolled_screen(5);
    assert_eq!(screen.scroll(Scroll::Delta(0)), None);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a motion toward the live tail from the live tail
/// reports no motion.
///
/// Case: the scroll-on-input policy snaps to the tail on a keystroke
/// typed while the viewport was already there.
#[test]
fn a_motion_toward_the_tail_from_the_tail_reports_nothing() {
    let mut screen = scrolled_screen(5);
    assert_eq!(screen.scroll(Scroll::Bottom), None);
    assert_eq!(screen.scroll(Scroll::PageDown), None);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a half page on a one-row screen is zero rows and so
/// reports no motion.
///
/// Case: the window is dragged down to a single row and the user
/// keeps pressing the half-page motion.
#[test]
fn a_half_page_on_a_one_row_screen_reports_no_motion() {
    let mut screen = Screen::new(GridSize { cols: 4, rows: 1 }, 10);
    for _ in 0..3 {
        screen.state.line = ScreenLine(0);
        screen.line_feed();
    }
    assert_eq!(screen.grid.history_len(), 3);
    assert_eq!(screen.scroll(Scroll::HalfPageUp), None);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a screen keeping no history never moves, whatever
/// motion it is handed.
///
/// Case: a full-screen application is showing, and the alternate
/// screen it runs on keeps no scrollback to move over.
#[test]
fn a_screen_without_history_never_moves() {
    let mut screen = Screen::new(GridSize { cols: 4, rows: 3 }, 0);
    for _ in 0..5 {
        screen.state.line = ScreenLine(2);
        screen.line_feed();
    }
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(screen.scroll(Scroll::Top), None);
    assert_eq!(screen.scroll(Scroll::PageUp), None);
    assert_eq!(screen.scroll(Scroll::Delta(9)), None);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}
