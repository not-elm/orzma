//! Tests for column addressing.

use super::*;

/// Asserts that a one-based column parameter lands on the zero-based
/// cell of the row the cursor already occupies.
///
/// Case: a full-screen application redraws a status field by jumping
/// to column 6 of the row it is already writing.
#[test]
fn a_one_based_column_lands_on_the_zero_based_cell_of_the_same_row() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(0);
    screen.move_cursor_to_column(Some(6));
    assert_eq!(screen.state.column, GridColumn(5));
    assert_eq!(screen.state.line, ScreenLine(2));
}

/// Asserts that an omitted parameter addresses the first column,
/// leaving the row untouched.
///
/// Case: an application emits a bare `CSI G` to return to the left
/// edge of the row it is drawing.
#[test]
fn an_omitted_parameter_addresses_the_first_column() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(7);
    screen.move_cursor_to_column(None);
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that addressing a column leaves the cursor's row unchanged
/// even while origin mode makes the seating helper's line argument
/// relative to the top margin.
///
/// Case: an application reserves rows 2 through 4 as a pane, turns on
/// origin mode, and moves along a row inside that pane.
#[test]
fn a_column_move_under_a_margin_origin_leaves_the_row_where_it_was() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(0);
    screen.move_cursor_to_column(Some(3));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that addressing a column leaves a cursor sitting above the
/// top margin on its row rather than pulling it into the region.
///
/// Case: an application keeps a scrolling pane on rows 2 and 3 but
/// moves along the header row above it to update a title.
#[test]
fn a_column_move_above_the_top_margin_leaves_the_row_where_it_was() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to_column(Some(2));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that addressing a column leaves a cursor sitting below the
/// bottom margin on its row rather than pulling it into the region.
///
/// Case: an application keeps a scrolling pane on rows 2 and 3 and
/// moves along the status row below it.
#[test]
fn a_column_move_below_the_bottom_margin_leaves_the_row_where_it_was() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to_column(Some(2));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that a column past the right edge stops at the last column
/// rather than being refused.
///
/// Case: an application sized for an 80-column window addresses
/// column 80 after the user shrinks the pane to four columns.
#[test]
fn a_column_past_the_right_edge_clamps_to_the_last_column() {
    let mut screen = screen();
    screen.move_cursor_to_column(Some(80));
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that the largest representable column parameter clamps to
/// the last column without overflowing the one-based conversion.
///
/// Case: a program emits a wildly out-of-range `CSI 999999 G`, which
/// the parameter decoder saturates to `u16::MAX`.
#[test]
fn the_largest_column_parameter_clamps_without_overflowing() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(0);
    screen.move_cursor_to_column(Some(u16::MAX));
    assert_eq!(screen.state.column, GridColumn(19));
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that a zero addresses the first column, the same as a one,
/// rather than underflowing the zero-based conversion.
///
/// Case: a program that builds its sequences from zero-based variables
/// emits `CSI 0 G`.
#[test]
fn a_zero_addresses_the_first_column() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(7);
    screen.move_cursor_to_column(Some(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that addressing a column discards a pending deferred wrap
/// rather than preserving it as a linefeed does.
///
/// Case: an application fills a row to its last column and then jumps
/// back along that row instead of printing again.
#[test]
fn addressing_a_column_disarms_the_deferred_wrap() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(19);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_column(Some(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that addressing the column the cursor already occupies
/// still discards a pending deferred wrap.
///
/// Case: an application fills a row to its last column and then
/// addresses that very column again before printing.
#[test]
fn addressing_the_column_already_held_still_disarms_the_deferred_wrap() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(19);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_column(Some(20));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.column, GridColumn(19));
}

/// Asserts that a row restored above the current top margin is left
/// where it is rather than reseated onto that margin.
///
/// Case: an application saves the cursor with origin mode on, moves the
/// scroll region down, restores the cursor, and then addresses a column.
#[test]
fn a_row_restored_above_the_top_margin_is_preserved() {
    let mut screen = tall_screen();
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.save_checkpoint();
    screen.set_scroll_region(Some(2), Some(4));
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(0));
    screen.move_cursor_to_column(Some(3));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a row restored below the current bottom margin is left
/// where it is rather than clamped onto that margin.
///
/// Case: an application saves the cursor on the last row with origin
/// mode on, shrinks the scroll region, restores the cursor, and then
/// addresses a column.
#[test]
fn a_row_restored_below_the_bottom_margin_is_preserved() {
    let mut screen = tall_screen();
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.move_cursor_to(Some(4), Some(1));
    screen.save_checkpoint();
    screen.set_scroll_region(Some(1), Some(3));
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(3));
    screen.move_cursor_to_column(Some(2));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(1));
}
