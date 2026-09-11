//! Tests for the carriage return.

use super::*;

/// Asserts that a carriage return rewinds the column and clears
/// the deferred-wrap flag.
///
/// Case: a shell prints a partial line and returns to overwrite it,
/// as progress indicators do with a bare `\r`.
#[test]
fn carriage_return_rewinds_and_clears_pending_wrap() {
    let mut screen = screen();
    screen.state.column = GridColumn(2);
    screen.state.pending_wrap = true;
    screen.carriage_return();
    assert_eq!(screen.state.column, GridColumn(0));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that a carriage return at column zero still disarms
/// a pending deferred wrap.
///
/// Case: a one-column screen prints a character, which arms the
/// wrap without ever leaving column zero, and the application then
/// emits a bare `\r`.
#[test]
fn a_carriage_return_at_column_zero_disarms_a_pending_wrap() {
    let mut screen = Screen::new(GridSize { cols: 1, rows: 3 }, 10);
    screen.print('x', InsertReplaceMode::Replace);
    assert_eq!(screen.state.column, GridColumn(0));
    assert!(screen.state.pending_wrap);
    screen.carriage_return();
    assert!(!screen.state.pending_wrap);
}
