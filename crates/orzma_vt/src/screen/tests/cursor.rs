//! Tests for the cursor snapshot a screen reports.

use super::*;

/// Asserts that the reported cursor carries the write position, and
/// that the caller's DECTCEM state decides visibility while shape and
/// blink stay at the terminal's power-up values.
///
/// Case: a shell prints its prompt and the caller asks for the caret
/// shown after it. The same screen, mid-repaint, has a full-screen
/// application asking the caller to report the caret hidden instead.
#[test]
fn the_cursor_reports_the_write_position_and_the_callers_visibility() {
    let mut screen = screen();
    screen.print('a');
    screen.print('b');

    let shown = screen.cursor(TextCursorEnable::Shown);
    assert_eq!(shown.point.line, GridLine(0));
    assert_eq!(shown.point.column, GridColumn(2));
    assert_eq!(shown.shape, CursorShape::Block);
    assert!(!shown.blinking);
    assert!(shown.visible);

    let hidden = screen.cursor(TextCursorEnable::Hidden);
    assert!(!hidden.visible);
    assert_eq!(hidden.point, shown.point);
    assert_eq!(hidden.shape, shown.shape);
}
