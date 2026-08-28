//! Tests for the shared tabulation-move helper.

use super::*;

/// Asserts that a tab seats the cursor at the target column.
///
/// Case: the shell emits a tab while listing a directory in
/// aligned columns.
#[test]
fn a_tab_seats_the_cursor_at_the_target_column() {
    let mut screen = screen();
    screen.tab_to(GridColumn(2));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that seating the cursor leaves an armed deferred
/// wrap alone.
///
/// The agreed policy preserves the flag, unlike
/// [`Screen::carriage_return`]. Disarming it would seat the
/// cursor back onto the row the application had already filled,
/// which is the behaviour both VTE and Windows Terminal found
/// real DEC hardware never had.
///
/// Case: an application fills a row to its last cell and then
/// emits a tab instead of more text.
#[test]
fn a_tab_keeps_the_deferred_wrap_armed() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    assert!(screen.state.pending_wrap);
    screen.tab_to(GridColumn(0));
    assert!(screen.state.pending_wrap);
}
