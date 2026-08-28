//! Tests for the scrollback display offset.

use super::*;

/// Asserts that output arriving while the user is scrolled back
/// leaves the viewed content where it was.
///
/// The agreed policy holds the viewport still rather than letting
/// it drift with the live tail: DECSET 1010 (`scrollTtyOutput`)
/// stays off, so the viewport holds its position while the PTY
/// emits output, and every terminal that keeps scrollback
/// behaves this way.
///
/// Case: the user is reading an earlier command's output when a
/// background build prints its next line.
#[test]
fn output_below_a_scrolled_viewport_holds_the_view_still() {
    let mut screen = screen();
    screen.grid[ScreenLine(0)][0].c = 'a';
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.viewport.offset = DisplayOffset(1);
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');

    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.display_offset(), DisplayOffset(2));
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
}

/// Asserts that a scroll inside a region below a non-zero top
/// margin leaves a scrolled-back viewport's offset untouched.
///
/// The offset counts rows of scrollback, and such a scroll adds
/// none, so advancing it would slide the view a row further back
/// than the user put it.
///
/// Case: the user is reading scrollback while a full-screen
/// application with a pinned header scrolls its pane.
#[test]
fn a_scroll_that_adds_no_history_leaves_the_offset_alone() {
    let mut screen = tall_screen();
    for _ in 0..3 {
        screen.state.line = ScreenLine(3);
        screen.line_feed();
    }
    assert_eq!(screen.grid.history_len(), 3);
    screen.viewport.offset = DisplayOffset(1);

    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    screen.state.line = ScreenLine(3);
    screen.line_feed();
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}

/// Asserts that output at the live tail leaves the viewport pinned
/// there.
///
/// Case: an unscrolled terminal keeps printing, and the window has
/// to follow the newest line rather than freeze.
#[test]
fn output_at_the_live_tail_keeps_the_viewport_pinned() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a scroll at history capacity clamps the offset
/// instead of naming a row the ring no longer holds.
///
/// The agreed policy accepts that the view drifts once scrollback
/// is full: the row the user was reading has been evicted, so there
/// is nothing left to hold still on.
///
/// Case: the user is parked at the top of a full scrollback while
/// output keeps arriving.
#[test]
fn a_scroll_at_history_capacity_clamps_the_offset() {
    let mut screen = Screen::new(GridSize { cols: 4, rows: 3 }, 1);
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.viewport.offset = DisplayOffset(1);
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}
