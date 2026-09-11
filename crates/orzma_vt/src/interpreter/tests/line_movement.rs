//! Tests for the control functions that move the cursor down a line,
//! and for the scroll region they move against.

use super::*;

/// Asserts that `ESC D` moves the cursor down a row and leaves the
/// column where it stood.
///
/// Case: a full-screen program walks down one column of a form,
/// emitting the seven-bit index between fields.
#[test]
fn the_seven_bit_index_keeps_the_column() {
    let device = interpret(b"a\x1bDb");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[1].c,
        'b'
    );
}

/// Asserts that `ESC E` moves the cursor down a row and returns the
/// carriage.
///
/// Case: a program ends a log line with the seven-bit next line.
#[test]
fn the_seven_bit_next_line_returns_the_carriage() {
    let device = interpret(b"a\x1bEb");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'b'
    );
}

/// Asserts that a scroll region set by `CSI r` is what a later
/// linefeed scrolls against.
///
/// Case: a full-screen application reserves the last row for a
/// status line and fills the pane above it.
#[test]
fn a_scroll_region_reaches_the_linefeed() {
    let device = interpret(b"\x1b[1;2ra\n\nb");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[1].c,
        'b'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(2))[1].c,
        ' '
    );
}
