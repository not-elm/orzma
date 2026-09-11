//! Tests for erasure within one line.

use super::*;

/// Asserts that erase-to-end clears from the cursor to the right
/// edge with the pen background.
///
/// Case: an application with a colored background truncates the
/// tail of a line with `EL 0`.
#[test]
fn erase_to_end_clears_from_the_cursor_with_the_pen_background() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.state.column = GridColumn(1);
    screen.pen_mut().bg = Color::Indexed(2);
    let damage = screen.erase_in_line(EraseLineMode::ToEnd);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(2));
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that erase-to-start clears through the cursor column
/// inclusively.
///
/// The agreed convention matches `EL 1`: the erased span is
/// `0..=cursor.column`, the classic off-by-one of this operation.
///
/// Case: an application rewrites the head of a line and clears
/// what it had written so far, cursor included.
#[test]
fn erase_to_start_includes_the_cursor_column() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.state.column = GridColumn(1);
    screen.erase_in_line(EraseLineMode::ToStart);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][2].c, 'c');
}

/// Asserts that erase-to-end is a no-op while the deferred wrap is
/// armed.
///
/// The agreed policy follows alacritty: with the wrap pending the
/// cursor logically sits past the row's last cell, so `EL 0`
/// erases nothing rather than the just-printed last cell.
///
/// Case: an application fills a row to its last column and then
/// issues `EL 0` before printing anything further.
#[test]
fn erase_to_end_is_a_no_op_under_pending_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    let damage = screen.erase_in_line(EraseLineMode::ToEnd);
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'd');
    assert_eq!(damage, None);
}

/// Asserts that erase-all clears the whole row regardless of the
/// cursor column.
///
/// Case: a full-screen application repaints a status line in place
/// by clearing the entire row with `EL 2` before rewriting it.
#[test]
fn erase_all_clears_the_whole_row() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.state.column = GridColumn(1);
    screen.erase_in_line(EraseLineMode::All);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][2].c, ' ');
}
