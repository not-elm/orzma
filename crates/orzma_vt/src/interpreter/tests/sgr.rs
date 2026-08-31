//! Tests for select graphic rendition reaching the pen.

use super::*;

/// Asserts that `CSI m` reaches the pen, so a printed cell carries
/// the attributes the sequence selected.
///
/// Case: a build tool prints a red error message.
#[test]
fn the_select_graphic_rendition_sequence_reaches_the_pen() {
    let device = interpret(b"\x1b[31;1mx");
    let cell = device.active_screen().viewport_row(ViewportLine(0))[0];
    assert_eq!(cell.fg, Color::Indexed(1));
    assert!(cell.style.contains(Style::BOLD));
}

/// Asserts that the pen survives between sequences, so a run keeps
/// its attributes until something changes them.
///
/// Case: a program colours a word, prints it, and resets before the
/// rest of the line.
#[test]
fn the_pen_survives_between_sequences() {
    let device = interpret(b"\x1b[31ma\x1b[mb");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].fg, Color::Indexed(1));
    assert_eq!(row[1].fg, Color::DefaultForeground);
}
