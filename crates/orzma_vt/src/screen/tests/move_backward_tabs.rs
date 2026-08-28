//! Tests for backward tabulation.

use super::*;

/// Asserts that a backward tab seats the cursor on the previous
/// stop.
///
/// Case: the user presses Shift-Tab to step back to the
/// previous column of a form.
#[test]
fn cbt_moves_back_to_the_previous_stop() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(17);
    screen.move_backward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(16));
}

/// Asserts that a backward tab before the first stop lands on
/// column zero.
///
/// The agreed policy makes the left edge a fallback rather than
/// a stop, because the reset stride leaves column zero empty.
///
/// Case: the user presses Shift-Tab near the start of a line,
/// before the first tab position.
#[test]
fn cbt_before_the_first_stop_clamps_to_column_zero() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(5);
    screen.move_backward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(0));
}
