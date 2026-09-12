//! Tests for the backspace.

use super::*;

/// Asserts that a backspace steps the cursor one column left.
///
/// Case: a shell line editor erases the character the user just
/// typed, moving left before overwriting it with a space.
#[test]
fn backspace_moves_the_cursor_one_column_left() {
    let mut screen = screen();
    screen.state.column = GridColumn(2);
    screen.backspace();
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that a backspace at column zero leaves the cursor
/// where it is rather than wrapping back onto the previous row.
///
/// Case: a program emits more backspaces than it printed
/// characters, running past the start of the line.
#[test]
fn a_backspace_at_column_zero_does_not_move() {
    let mut screen = screen();
    screen.backspace();
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that a backspace after a full row both steps back and
/// disarms the deferred wrap, landing one column short of the cell
/// just written rather than on it.
///
/// Case: an application fills a row to its last cell and then
/// backs up to overwrite the character before the last one.
#[test]
fn a_backspace_after_a_full_row_steps_back_and_disarms_the_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert!(screen.state.pending_wrap);
    screen.backspace();
    assert_eq!(screen.state.column, GridColumn(2));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that a backspace at column zero still disarms a
/// pending deferred wrap.
///
/// Case: a one-column screen prints a character, which arms the
/// wrap without ever leaving column zero, and the application
/// then emits a backspace.
#[test]
fn a_backspace_at_column_zero_disarms_a_pending_wrap() {
    let mut screen = Screen::new(GridSize { cols: 1, rows: 3 }, 10);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert!(screen.state.pending_wrap);
    screen.backspace();
    assert_eq!(screen.state.column, GridColumn(0));
    assert!(!screen.state.pending_wrap);
}
