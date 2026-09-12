//! Tests for the cursor snapshot a screen reports.

use super::*;
use crate::device::modes::{CursorBlink, CursorShape, TextCursorModes};

/// Asserts that the reported cursor carries the write position and the
/// presentation handed in, the screen contributing only the position.
///
/// Case: a shell prints its prompt and the caller asks for the caret
/// the terminal starts with. The same screen, mid-repaint, has a
/// full-screen application asking for a hidden blinking bar instead.
#[test]
fn the_cursor_reports_the_write_position_and_the_callers_presentation() {
    let mut screen = screen();
    screen.print('a', InsertReplaceMode::Replace, AutoWrap::Enabled);
    screen.print('b', InsertReplaceMode::Replace, AutoWrap::Enabled);

    let shown = screen.cursor(TextCursorModes::default());
    assert_eq!(shown.point.line, GridLine(0));
    assert_eq!(shown.point.column, GridColumn(2));
    assert_eq!(shown.shape, CursorShape::Block);
    assert!(!shown.blinking);
    assert!(shown.visible);

    let hidden = screen.cursor(TextCursorModes {
        enable: TextCursorEnable::Hidden,
        shape: CursorShape::Bar,
        blink: CursorBlink::Blinking,
    });
    assert_eq!(hidden.point, shown.point);
    assert!(!hidden.visible);
    assert_eq!(hidden.shape, CursorShape::Bar);
    assert!(hidden.blinking);
}
