//! Tests for line addressing.

use super::*;

/// Asserts that a one-based line parameter lands on the zero-based row
/// while the cursor keeps the column it already occupies.
///
/// Case: an application redraws a column of a table by jumping to
/// row 3 without disturbing its horizontal position.
#[test]
fn a_one_based_line_lands_on_the_zero_based_row_of_the_same_column() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to_line(Some(3));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that an omitted parameter addresses the first line,
/// leaving the column untouched.
///
/// Case: an application emits a bare `CSI d` to return to the top row
/// of the column it is drawing.
#[test]
fn an_omitted_parameter_addresses_the_first_line() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(None);
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that an explicit one addresses the first line.
///
/// Case: an application that always writes its parameters out emits
/// `CSI 1 d` instead of a bare `CSI d`.
#[test]
fn an_explicit_one_addresses_the_first_line() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.move_cursor_to_line(Some(1));
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that a line below the last row stops on the last line,
/// clamped against the row count rather than the column count.
///
/// Case: an application sized for a taller window addresses row 40
/// after the user shrinks the pane to three rows.
#[test]
fn a_line_below_the_last_row_stops_on_the_last_line() {
    let mut screen = screen();
    screen.move_cursor_to_line(Some(40));
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that the line is measured from the top margin while the
/// origin is within the margins, and the column is untouched.
///
/// Case: an application with a reserved header turns on origin mode
/// and addresses the first row of its own pane.
#[test]
fn a_margin_origin_measures_the_line_from_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(1));
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that an interior line is measured from the top margin
/// rather than clamped into the region as an absolute row.
///
/// Case: an application with a reserved header turns on origin mode
/// and addresses the second row of its own pane.
#[test]
fn a_margin_origin_measures_an_interior_line_from_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(2));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a line past the scroll region clamps to the bottom
/// margin, not to the last row, while the origin is within the margins.
///
/// Case: an application with origin mode on addresses a row below the
/// three-row pane it reserved for itself.
#[test]
fn a_line_past_the_region_clamps_to_the_bottom_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.move_cursor_to_line(Some(9));
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that the line is absolute and reaches above the top margin
/// while the origin is the upper-left corner.
///
/// Case: an application keeps a scrolling pane on rows 2 through 4 but
/// addresses the header row above it to update a title.
#[test]
fn an_upper_left_origin_reaches_a_line_above_the_margins() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(1));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that the line reaches below the bottom margin while the
/// origin is the upper-left corner, bounded only by the last row.
///
/// Case: an application keeps a scrolling pane on rows 2 and 3 and
/// addresses the status row below it.
#[test]
fn an_upper_left_origin_reaches_a_line_below_the_margins() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(4));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that an omitted parameter under a margin origin reaches the
/// top margin rather than the top of the screen.
///
/// Case: an application with origin mode on emits a bare `CSI d`.
#[test]
fn an_omitted_parameter_under_a_margin_origin_addresses_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(None);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that the largest representable line parameter clamps to the
/// bottom margin without overflowing the origin addition.
///
/// Case: a program emits a wildly out-of-range `CSI 999999 d` while
/// origin mode is on, which the parameter decoder saturates to
/// `u16::MAX`.
#[test]
fn the_largest_line_parameter_clamps_without_overflowing() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(3), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(u16::MAX));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a zero addresses the first line, the same as a one,
/// rather than underflowing the zero-based conversion.
///
/// Case: a program that builds its sequences from zero-based variables
/// emits `CSI 0 d`.
#[test]
fn a_zero_addresses_the_first_line() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.move_cursor_to_line(Some(0));
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that addressing a line discards a pending deferred wrap
/// rather than preserving it.
///
/// Case: an application fills a row to its last column and then jumps
/// to another row instead of printing again.
#[test]
fn addressing_a_line_disarms_the_deferred_wrap() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_line(Some(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that addressing the line the cursor already occupies still
/// discards a pending deferred wrap.
///
/// Case: an application fills a row to its last column and then
/// addresses that very row again before printing.
#[test]
fn addressing_the_line_already_held_still_disarms_the_deferred_wrap() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_line(Some(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.line, ScreenLine(2));
}
