//! Tests for what reaches a cell when a character is printed, and what
//! the printing path drops before it gets there.

use super::*;

/// Asserts that DEL neither reaches a cell nor advances the cursor.
///
/// Case: a program pads a fixed-width record with DEL, as a paper
/// tape editor does to strike a character out.
#[test]
fn delete_prints_nothing() {
    let device = interpret(b"a\x7fb");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        'b'
    );
}

/// Asserts that DEL does not spend a pending single shift.
///
/// Case: a program pads with DEL between emitting `SS2` and the box
/// character the shift was meant for.
#[test]
fn delete_leaves_a_pending_single_shift_armed() {
    let device = interpret(b"\x1b*0\x1bN\x7fq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        '─'
    );
}
