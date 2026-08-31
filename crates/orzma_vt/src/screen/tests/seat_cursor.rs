//! Tests for the shared cursor-seating helper.

use super::*;

/// Asserts that with the origin at the upper-left corner a
/// relative line is an absolute one.
///
/// Case: a full-screen application addresses the third row of an
/// unrestricted screen.
#[test]
fn an_upper_left_origin_leaves_the_line_absolute() {
    let mut screen = tall_screen();
    screen.seat_cursor(ScreenLine(2), GridColumn(1));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that with the origin within the margins a relative
/// line is measured from the top margin.
///
/// Case: an application pins a header on the first row, turns on
/// origin mode, and addresses the first row of its own pane.
#[test]
fn a_margin_origin_measures_from_the_top_margin() {
    let mut screen = tall_screen();
    screen
        .scroll_region
        .set_margins(Margins::resolve(Some(2), Some(4), 4).expect("2..=4 is a legal region"));
    screen
        .scroll_region
        .set_origin_mode(OriginMode::WithinMargins);
    screen.seat_cursor(ScreenLine(0), GridColumn(0));
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that a line past the bottom margin clamps to it while
/// the origin is within the margins.
///
/// Case: an application with origin mode on addresses a row
/// below the pane it reserved for itself.
#[test]
fn a_line_past_the_bottom_margin_clamps_to_it() {
    let mut screen = tall_screen();
    screen
        .scroll_region
        .set_margins(Margins::resolve(Some(1), Some(3), 4).expect("1..=3 is a legal region"));
    screen
        .scroll_region
        .set_origin_mode(OriginMode::WithinMargins);
    screen.seat_cursor(ScreenLine(9), GridColumn(0));
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that a line past the last row clamps to it while the
/// origin is the upper-left corner.
///
/// Case: an application sized for a taller window addresses row
/// 40 of a four-row screen.
#[test]
fn a_line_past_the_last_row_clamps_to_it() {
    let mut screen = tall_screen();
    screen.seat_cursor(ScreenLine(39), GridColumn(0));
    assert_eq!(screen.state.line, ScreenLine(3));
}

/// Asserts that a column past the right edge clamps to the last
/// column.
///
/// Case: an application sized for a wider window addresses
/// column 80 of a four-column screen.
#[test]
fn a_column_past_the_right_edge_clamps() {
    let mut screen = tall_screen();
    screen.seat_cursor(ScreenLine(0), GridColumn(79));
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that seating the cursor discards a pending deferred
/// wrap rather than preserving it as a linefeed does.
///
/// Case: an application fills a row to its last column and then
/// addresses a cell elsewhere instead of printing again.
#[test]
fn seating_the_cursor_disarms_the_deferred_wrap() {
    let mut screen = tall_screen();
    screen.state.pending_wrap = true;
    screen.seat_cursor(ScreenLine(0), GridColumn(0));
    assert!(!screen.state.pending_wrap);
}
