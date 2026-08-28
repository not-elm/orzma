//! Tests for reading a row through the viewport.

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
