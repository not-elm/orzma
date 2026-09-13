//! OSC 8 hyperlink handling.

use super::*;

/// Asserts that the cells printed inside an `OSC 8` carry its id and the
/// cells after the close carry none.
///
/// Case: a build tool prints a clickable path in the middle of an error
/// line and returns to plain text afterwards.
#[test]
fn cells_printed_inside_a_hyperlink_carry_its_id() {
    let device = interpret(b"a\x1b]8;;https://a.example\x1b\\b\x1b]8;;\x1b\\c");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].hyperlink_id, None);
    assert!(row[1].hyperlink_id.is_some());
    assert_eq!(row[2].hyperlink_id, None);
}

/// Asserts that two runs naming one id and one target share an id.
///
/// Case: a long link wraps, so the program closes and reopens it with
/// the same `id=` on the next line.
#[test]
fn two_runs_naming_one_id_share_it() {
    let device = interpret(
        b"\x1b]8;id=7;https://a.example\x1b\\a\x1b]8;;\x1b\\\x1b]8;id=7;https://a.example\x1b\\b\x1b]8;;\x1b\\",
    );
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
    assert_eq!(row[1].hyperlink_id, row[0].hyperlink_id);
}

/// Asserts that two runs naming one target without an id take separate
/// ids.
///
/// Case: `ls --hyperlink=auto` lists one file twice and tags neither
/// listing with an id.
#[test]
fn two_runs_without_an_id_take_separate_ids() {
    let device = interpret(
        b"\x1b]8;;https://a.example\x1b\\a\x1b]8;;\x1b\\\x1b]8;;https://a.example\x1b\\b\x1b]8;;\x1b\\",
    );
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
    assert!(row[1].hyperlink_id.is_some());
    assert_ne!(row[0].hyperlink_id, row[1].hyperlink_id);
}

/// Asserts that a truncated hyperlink sequence leaves the open link
/// alone.
///
/// Case: a program's output is cut off mid-sequence while it is partway
/// through printing a clickable path.
#[test]
fn a_truncated_hyperlink_sequence_leaves_the_open_link_alone() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\a\x1b]8\x1b\\b");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
    assert_eq!(row[1].hyperlink_id, row[0].hyperlink_id);
}

/// Asserts that a full attribute reset leaves the hyperlink the cursor
/// paints with untouched.
///
/// Case: a program prints a coloured link, ends the colour with
/// `ESC[0m`, and prints one more character before closing the link.
#[test]
fn a_full_attribute_reset_keeps_the_hyperlink() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\a\x1b[0mb");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
    assert_eq!(row[1].hyperlink_id, row[0].hyperlink_id);
}
