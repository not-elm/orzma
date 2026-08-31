//! Tests for cursor addressing.

use super::*;

/// Asserts that omitted parameters address the first line and
/// column.
///
/// Case: an application homes the cursor with a bare `CSI H`.
#[test]
fn omitted_parameters_address_the_first_cell() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to(None, None);
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that a zero addresses the first line and column, the
/// same as a one.
///
/// The agreed policy follows VT510 p.116 — "If Pl or Pc is not
/// selected or selected as 0, then the cursor moves to the first
/// line or column".
///
/// Case: a program that builds its sequences from zero-based
/// variables emits `CSI 0 ; 0 H`.
#[test]
fn a_zero_addresses_the_first_cell() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(2);
    screen.move_cursor_to(Some(0), Some(0));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that one-based parameters land on zero-based cells.
///
/// Case: a full-screen application draws a box corner by
/// addressing row 3, column 2.
#[test]
fn one_based_parameters_land_on_zero_based_cells() {
    let mut screen = tall_screen();
    screen.move_cursor_to(Some(3), Some(2));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that the line is measured from the top margin while
/// the origin is within the margins.
///
/// Case: an application with a reserved header turns on origin
/// mode and addresses the first row of its own pane.
#[test]
fn a_margin_origin_measures_the_line_from_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to(Some(1), Some(1));
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that the line is absolute and reaches outside the
/// margins while the origin is the upper-left corner.
///
/// The agreed policy follows VT510 p.195: with `DECOM` reset the
/// line numbering is independent of the margins and the cursor
/// can move outside them.
///
/// Case: an application keeps a scrolling pane but addresses the
/// header row above it to update a title.
#[test]
fn an_upper_left_origin_reaches_outside_the_margins() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to(Some(1), Some(1));
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that a line past the region clamps to the bottom
/// margin while the origin is within the margins.
///
/// Case: an application with origin mode on addresses a row
/// below the pane it reserved for itself.
#[test]
fn a_line_past_the_region_clamps_to_the_bottom_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.move_cursor_to(Some(9), Some(1));
    assert_eq!(screen.state.line, ScreenLine(2));
}
