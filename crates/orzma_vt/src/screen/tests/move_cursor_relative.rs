//! Tests for the relative cursor motions.

use super::*;

/// Builds a four-row screen whose scrolling region is rows 1 through 2,
/// leaving row 0 above it and row 3 below it.
fn regioned_screen() -> Screen {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    assert_eq!(screen.scroll_region.top_margin(), ScreenLine(1));
    assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(2));
    screen
}

/// Asserts that a cursor inside the region stops at the top margin
/// rather than running on to the first row.
///
/// Case: a full-screen editor with a pinned header moves the cursor up
/// past the top of its text pane.
#[test]
fn a_cursor_up_inside_the_region_stops_at_the_top_margin() {
    let mut screen = regioned_screen();
    screen.state.line = ScreenLine(2);
    screen.move_cursor_up(5);
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that a cursor below the region also stops at the top
/// margin, because the margin lies on the way.
///
/// Case: an application parks the cursor on a status line under its
/// pane and then moves it back up into the pane.
#[test]
fn a_cursor_up_below_the_region_stops_at_the_top_margin() {
    let mut screen = regioned_screen();
    screen.state.line = ScreenLine(3);
    screen.move_cursor_up(5);
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that a cursor already above the region reaches the first
/// row, the margin being behind it.
///
/// Case: an application writes into a header above its pane and moves
/// the cursor to the very top of the screen.
#[test]
fn a_cursor_up_above_the_region_reaches_the_first_row() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(3), Some(4));
    screen.state.line = ScreenLine(1);
    screen.move_cursor_up(5);
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that a cursor inside the region stops at the bottom margin
/// rather than running on to the last row.
///
/// Case: a full-screen editor moves the cursor down past the bottom of
/// its text pane, above a reserved status line.
#[test]
fn a_cursor_down_inside_the_region_stops_at_the_bottom_margin() {
    let mut screen = regioned_screen();
    screen.state.line = ScreenLine(1);
    screen.move_cursor_down(5);
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that a cursor above the region also stops at the bottom
/// margin, mirroring the upward case.
///
/// Case: an application writes a header, then moves the cursor down
/// into its pane.
#[test]
fn a_cursor_down_above_the_region_stops_at_the_bottom_margin() {
    let mut screen = regioned_screen();
    screen.state.line = ScreenLine(0);
    screen.move_cursor_down(5);
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that a cursor already below the region reaches the last
/// row.
///
/// Case: an application addresses its status line and moves the cursor
/// to the very bottom of the screen.
#[test]
fn a_cursor_down_below_the_region_reaches_the_last_row() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(2));
    screen.state.line = ScreenLine(2);
    screen.move_cursor_down(5);
    assert_eq!(screen.state.line, ScreenLine(3));
}

/// Asserts that a leftward motion stops at the first column, the page
/// border rather than a margin.
///
/// Case: a shell redrawing a prompt walks the cursor further left than
/// the line is wide.
#[test]
fn a_cursor_left_stops_at_the_first_column() {
    let mut screen = tall_screen();
    screen.state.column = GridColumn(2);
    screen.move_cursor_left(9);
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that a rightward motion stops at the last column.
///
/// Case: a program indents past the width of a narrow window.
#[test]
fn a_cursor_right_stops_at_the_last_column() {
    let mut screen = tall_screen();
    screen.move_cursor_right(9);
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that the largest representable count saturates on every
/// axis rather than wrapping.
///
/// Case: a program emits `CSI 65535 B` after computing a motion from a
/// value it never bounded.
#[test]
fn the_largest_count_saturates_on_every_axis() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(1);
    screen.move_cursor_down(u16::MAX);
    assert_eq!(screen.state.line, ScreenLine(3));
    screen.move_cursor_right(u16::MAX);
    assert_eq!(screen.state.column, GridColumn(3));
    screen.move_cursor_up(u16::MAX);
    assert_eq!(screen.state.line, ScreenLine(0));
    screen.move_cursor_left(u16::MAX);
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that a motion that moves nothing still disarms the deferred
/// wrap.
///
/// Case: an application fills the last column and then asks for a
/// rightward motion the page border refuses.
#[test]
fn a_motion_at_the_boundary_still_disarms_the_deferred_wrap() {
    let mut screen = tall_screen();
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.move_cursor_right(1);
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that a relative motion under origin mode keeps the cursor
/// inside the region, which is where setting the mode seated it.
///
/// Case: a full-screen application sets origin mode and its pane, then
/// walks the cursor to the extremes of that pane.
#[test]
fn a_relative_motion_under_origin_mode_stays_inside_the_region() {
    let mut screen = regioned_screen();
    screen.set_origin_mode(OriginMode::WithinMargins);
    assert_eq!(screen.state.line, ScreenLine(1));
    screen.move_cursor_down(9);
    assert_eq!(screen.state.line, ScreenLine(2));
    screen.move_cursor_up(9);
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that a backspace lands where a one-column leftward motion
/// does, the two sharing one primitive.
///
/// Case: a shell erases the last character the user typed.
#[test]
fn a_backspace_matches_a_one_column_cursor_back() {
    let mut screen = tall_screen();
    screen.state.column = GridColumn(2);
    screen.backspace();
    let after_backspace = screen.state.column;
    screen.state.column = GridColumn(2);
    screen.move_cursor_left(1);
    assert_eq!(screen.state.column, after_backspace);
}
