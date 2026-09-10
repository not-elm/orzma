//! Tests for the character-editing CSI sequences: insert character and
//! delete character.

use super::*;

/// Asserts that `CSI @` inserts one blank at the cursor, shifts the rest
/// of the row right, and leaves the cursor on the column it opened.
///
/// Case: a shell inserts a character mid-command with the terminfo
/// `ich1` capability.
#[test]
fn the_insert_character_sequence_opens_one_blank_at_the_cursor() {
    let device = interpret(b"abcd\x1b[1;2H\x1b[@");
    let screen = device.active_screen();
    let row = screen.viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, 'a');
    assert_eq!(row[1].c, ' ');
    assert_eq!(row[2].c, 'b');
    assert_eq!(row[3].c, 'c');
    assert_eq!(screen.cursor_column(), GridColumn(1));
}

/// Asserts that `CSI Pn @` inserts `Pn` blanks.
///
/// Case: an editor opens a three-column gap at the start of a row with
/// the terminfo `ich` capability.
#[test]
fn the_insert_character_sequence_honors_its_count() {
    let device = interpret(b"abcd\x1b[1;1H\x1b[3@");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, ' ');
    assert_eq!(row[1].c, ' ');
    assert_eq!(row[2].c, ' ');
    assert_eq!(row[3].c, 'a');
}

/// Asserts that `CSI 0 @` inserts one blank rather than none, the zero
/// parameter reading as the default.
///
/// Case: an application emits an explicit zero where it means the
/// default count.
#[test]
fn a_zero_parameter_insert_character_sequence_inserts_one_blank() {
    let device = interpret(b"abcd\x1b[1;1H\x1b[0@");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, ' ');
    assert_eq!(row[1].c, 'a');
    assert_eq!(row[2].c, 'b');
    assert_eq!(row[3].c, 'c');
}

/// Asserts that `CSI P` deletes the cell at the cursor, shifts the rest
/// of the row left, and blanks the last column.
///
/// Case: a shell deletes a character mid-command with the terminfo
/// `dch1` capability.
#[test]
fn the_delete_character_sequence_removes_the_cell_at_the_cursor() {
    let device = interpret(b"abcd\x1b[1;2H\x1b[P");
    let screen = device.active_screen();
    let row = screen.viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, 'a');
    assert_eq!(row[1].c, 'c');
    assert_eq!(row[2].c, 'd');
    assert_eq!(row[3].c, ' ');
    assert_eq!(screen.cursor_column(), GridColumn(1));
}

/// Asserts that `CSI Pn P` deletes `Pn` cells.
///
/// Case: an editor removes a three-column indent from the start of a row
/// with the terminfo `dch` capability.
#[test]
fn the_delete_character_sequence_honors_its_count() {
    let device = interpret(b"abcd\x1b[1;1H\x1b[3P");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, 'd');
    assert_eq!(row[1].c, ' ');
    assert_eq!(row[2].c, ' ');
    assert_eq!(row[3].c, ' ');
}

/// Asserts that `CSI 0 P` deletes one cell rather than none, the zero
/// parameter reading as the default.
///
/// Case: an application emits an explicit zero where it means the
/// default count.
#[test]
fn a_zero_parameter_delete_character_sequence_deletes_one_cell() {
    let device = interpret(b"abcd\x1b[1;1H\x1b[0P");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, 'b');
    assert_eq!(row[1].c, 'c');
    assert_eq!(row[2].c, 'd');
    assert_eq!(row[3].c, ' ');
}

/// Asserts that an insert character raises the chunk liveness, so the
/// shifted row reaches a frame.
///
/// Case: a shell's line editor inserts a character in a chunk that
/// prints nothing of its own and moves the cursor nowhere.
#[test]
fn an_insert_character_reports_damage() {
    assert!(liveness_after(b"abcd\x1b[1;2H", b"\x1b[@"));
}

/// Asserts that a delete character raises the chunk liveness, so the
/// closed-up row reaches a frame.
///
/// Case: a shell's line editor deletes a character in a chunk that
/// prints nothing of its own and moves the cursor nowhere.
#[test]
fn a_delete_character_reports_damage() {
    assert!(liveness_after(b"abcd\x1b[1;2H", b"\x1b[P"));
}
