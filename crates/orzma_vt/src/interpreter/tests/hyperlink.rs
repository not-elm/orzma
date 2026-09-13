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
/// Case: a program's output is cut off immediately after the hyperlink
/// command's number, before any parameter is written.
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

/// Asserts that a saved cursor restores the hyperlink that was open
/// when it was saved.
///
/// Case: a program saves the cursor inside a link, closes the link to
/// print a plain status word, and restores to carry on printing it.
#[test]
fn a_restored_cursor_brings_back_the_hyperlink() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\\x1b7\x1b]8;;\x1b\\a\x1b8b");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
}

/// Asserts that a soft reset closes the open hyperlink.
///
/// Case: a program leaves a link open and a later `tput init` issues a
/// soft reset before the next command's output.
#[test]
fn a_soft_reset_closes_the_hyperlink() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\a\x1b[!pb");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
    assert_eq!(row[1].hyperlink_id, None);
}

/// Asserts that a full reset closes the open hyperlink.
///
/// Case: a program leaves a link open and the user runs `reset`.
#[test]
fn a_full_reset_closes_the_hyperlink() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\\x1bca");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].hyperlink_id, None);
}

/// Asserts that erasing a linked cell leaves no hyperlink behind.
///
/// Case: a program prints a link and then clears the line to redraw it.
#[test]
fn erasing_a_linked_cell_drops_its_hyperlink() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\ab\x1b[H\x1b[K");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].hyperlink_id, None);
    assert_eq!(row[1].hyperlink_id, None);
}

/// Asserts that each screen keeps its own open hyperlink.
///
/// Case: a shell prints a link, a full-screen editor takes over the
/// alternate screen and prints plain text there, and the editor then
/// exits back to the shell.
#[test]
fn the_two_screens_keep_independent_hyperlinks() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\a\x1b[?1049hb\x1b[?1049lc");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert!(row[0].hyperlink_id.is_some());
    assert_eq!(row[1].hyperlink_id, row[0].hyperlink_id);
}

/// Asserts that a hyperlink survives the wrap at the right edge.
///
/// Case: a URL is longer than the window is wide, so it continues on the
/// next line.
#[test]
fn a_hyperlink_survives_an_automatic_wrap() {
    let device = interpret(b"\x1b]8;;https://a.example\x1b\\abcde");
    let first = device.active_screen().viewport_row(ViewportLine(0));
    let second = device.active_screen().viewport_row(ViewportLine(1));
    assert!(first[3].hyperlink_id.is_some());
    assert_eq!(second[0].hyperlink_id, first[3].hyperlink_id);
}
