//! Tests for the ANSI modes `SM` and `RM` set and reset.

use super::*;

/// Every glyph of the device's first visible row, left to right.
fn first_row_glyphs(device: &DeviceState) -> Vec<char> {
    device
        .active_screen()
        .viewport_row(ViewportLine(0))
        .iter()
        .map(|cell| cell.c)
        .collect()
}

/// Asserts that `CSI 4 h` selects insert mode on the device.
///
/// Case: a curses application on a terminal without `ich` announces an
/// insertion by entering insert mode before it prints.
#[test]
fn a_set_mode_four_selects_insert_mode() {
    let device = interpret(b"\x1b[4h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Insert);
}

/// Asserts that `CSI 4 l` returns the device to replace mode.
///
/// Case: the same application leaves insert mode as soon as the
/// insertion is done, which curses always pairs with entering it.
#[test]
fn a_reset_mode_four_selects_replace_mode() {
    let device = interpret(b"\x1b[4h\x1b[4l");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}

/// Asserts that a device that saw no mode sequence is in replace mode.
///
/// Case: a shell starts up and prints its prompt without ever touching
/// IRM.
#[test]
fn a_device_that_saw_no_mode_sequence_replaces() {
    let device = interpret(b"$ ");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}

/// Asserts that `CSI ? 4 h` leaves IRM alone, because the private marker
/// selects DECSCLM rather than the ANSI mode of the same number.
///
/// Case: a program enables smooth scrolling on a terminal that also
/// answers the ANSI mode numbered four.
#[test]
fn a_private_mode_four_does_not_select_insert_mode() {
    let device = interpret(b"\x1b[?4h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}

/// Asserts that an unimplemented mode number in the parameter list does
/// not stop the ones behind it from applying.
///
/// Case: an application sets keyboard action, insert/replace and
/// send/receive in one sequence, and this terminal answers only the
/// middle one.
#[test]
fn an_unimplemented_mode_number_does_not_hide_an_implemented_one() {
    let device = interpret(b"\x1b[2;4;12h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Insert);
}

/// Asserts that `RIS` returns IRM to replace mode.
///
/// Case: a program leaves insert mode set when it dies, and the user
/// resets the terminal to recover from the mess it left.
#[test]
fn a_reset_to_initial_state_returns_the_mode_to_replace() {
    let device = interpret(b"\x1b[4h\x1bc");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}

/// Asserts that switching to the alternate screen carries IRM across,
/// because the flip saves only what DECSC saves.
///
/// Case: a shell in insert mode launches a full-screen editor, which
/// enters the alternate screen.
#[test]
fn an_alternate_screen_flip_keeps_the_mode() {
    let device = interpret(b"\x1b[4h\x1b[?1049h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Insert);
}

/// Asserts that a character printed while `CSI 4 h` is in force shifts
/// the row right, so the mode the device carries reaches the screen's
/// print path rather than stopping at the mode snapshot.
///
/// Case: a curses application without `ich` enters insert mode and types
/// one character into the middle of a line it has already drawn.
#[test]
fn a_print_under_set_mode_four_shifts_the_row_right() {
    let device = interpret(b"abcd\x1b[2G\x1b[4hX");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'X', 'b', 'c']);
}

/// Asserts that a character printed after `CSI 4 l` overwrites the cell
/// at the cursor, so the print path reads the live mode instead of
/// assuming insert once IRM has been set.
///
/// Case: the same application leaves insert mode and keeps echoing over
/// the line it drew.
#[test]
fn a_print_under_reset_mode_four_overwrites_the_cell() {
    let device = interpret(b"abcd\x1b[2G\x1b[4h\x1b[4lX");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'X', 'c', 'd']);
}

/// Asserts that `DECRC` leaves IRM where the data stream last put it,
/// because `DECSC` does not save the mode.
///
/// Case: an application saves the cursor while in insert mode, leaves
/// insert mode, and restores the cursor before drawing further.
#[test]
fn a_cursor_restore_does_not_carry_the_mode_back() {
    let device = interpret(b"\x1b[4h\x1b7\x1b[4l\x1b8");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}
