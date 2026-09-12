//! Tests for forward tabulation.

use super::*;

/// Asserts that a tab seats the cursor on the next stop.
///
/// Case: the shell emits a tab at the start of a line while
/// printing aligned columns.
#[test]
fn ht_moves_to_the_next_stop() {
    let mut screen = wide_screen();
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(8));
}

/// Asserts that a tab past the last reachable stop lands on this
/// screen's own right edge rather than wrapping to the next line or
/// refusing the move.
///
/// Case: a twenty-column window shows text that has already run
/// past the last tab position it can display, and the shell
/// emits one more tab.
#[test]
fn ht_at_the_last_stop_clamps_to_the_screens_own_right_edge() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(16);
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(19));
}

/// Asserts that a screen too narrow to reach any stop clamps to
/// its last column.
///
/// Case: the user shrinks the window to four columns and the
/// shell keeps emitting tabs.
#[test]
fn ht_on_a_narrow_screen_clamps_without_reaching_any_stop() {
    let mut screen = screen();
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that a counted forward tab skips the stops in
/// between.
///
/// Case: an application emits `CSI 2 I` to jump two tab
/// positions in one step.
#[test]
fn cht_counts_multiple_stops() {
    let mut screen = wide_screen();
    screen.move_forward_tabs(2);
    assert_eq!(screen.state.column, GridColumn(16));
}
