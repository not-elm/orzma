//! Tests for the cursor snapshot a frame carries.

use super::*;

/// Asserts that the reported cursor carries the write position and
/// is visible.
///
/// The agreed placeholder is Block / steady / visible, matching what
/// a terminal starts at; `Cursor::default()` is deliberately not
/// used because its `visible` is `false`, which would hide the
/// caret until the first DECTCEM.
///
/// Case: a shell prints its prompt and the next frame has to show
/// the caret after it.
#[test]
fn the_cursor_reports_the_write_position_and_is_visible() {
    let mut screen = screen();
    screen.print('a');
    screen.print('b');
    let cursor = screen.cursor();
    assert_eq!(cursor.point.line, GridLine(0));
    assert_eq!(cursor.point.column, GridColumn(2));
    assert_eq!(cursor.shape, CursorShape::Block);
    assert!(!cursor.blinking);
    assert!(cursor.visible);
}
