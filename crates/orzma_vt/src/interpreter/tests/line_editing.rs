//! Tests for the line-editing and region-scrolling CSI sequences:
//! insert line, delete line, scroll up, and scroll down.

use super::*;

/// Asserts that `CSI L` opens one blank row at the cursor and homes
/// the cursor to column zero.
///
/// Case: an editor scrolls its buffer back by one line with the
/// terminfo `il1` capability.
#[test]
fn the_insert_line_sequence_opens_a_blank_row_at_the_cursor() {
    let device = interpret(b"ab\x1b[L");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, 'a');
    assert_eq!(screen.cursor_column(), GridColumn(0));
}

/// Asserts that `CSI Pn L` inserts `Pn` rows.
///
/// Case: an editor scrolls its buffer back by two lines with the
/// terminfo `il` capability.
#[test]
fn the_insert_line_sequence_honors_its_count() {
    let device = interpret(b"a\x1b[2L");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(2))[0].c, 'a');
}

/// Asserts that `CSI M` deletes the cursor row, moves the rows below
/// it up, and homes the cursor to column zero.
///
/// Case: an editor scrolls its buffer forward by one line with the
/// terminfo `dl1` capability.
#[test]
fn the_delete_line_sequence_removes_the_cursor_row() {
    let device = interpret(b"ab\x1b[2;1Hc\x1b[1;2H\x1b[M");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'c');
    assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, ' ');
    assert_eq!(screen.cursor_column(), GridColumn(0));
}

/// Asserts that `CSI Pn M` deletes `Pn` rows and that a zero reads as
/// the default of one.
///
/// Case: an editor scrolls its buffer forward by two lines with the
/// terminfo `dl` capability.
#[test]
fn the_delete_line_sequence_honors_its_count() {
    let device = interpret(b"a\x1b[2;1Hb\x1b[3;1Hc\x1b[1;1H\x1b[2M");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'c'
    );

    let device = interpret(b"a\x1b[2;1Hb\x1b[1;1H\x1b[0M");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'b'
    );
}

/// Asserts that `CSI Pn S` scrolls the region up by `Pn` rows without
/// moving the cursor.
///
/// Case: a program uses the terminfo `indn` capability to scroll a
/// pane forward two lines at once.
#[test]
fn the_scroll_up_sequence_scrolls_the_region_without_moving_the_cursor() {
    let device = interpret(b"\x1b[3;1Ha\x1b[1;2H\x1b[2S");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(screen.cursor_column(), GridColumn(1));
}

/// Asserts that `CSI T` scrolls the region down by one row and that a
/// lone zero reads as that same default.
///
/// Case: a program uses the terminfo `rin` capability to scroll a
/// pane back one line.
#[test]
fn the_scroll_down_sequence_scrolls_the_region_down() {
    let device = interpret(b"a\x1b[T");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );

    let device = interpret(b"a\x1b[0T");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );
}

/// Asserts that `CSI ^`, xterm's alternate spelling of SD, scrolls the
/// region down by one row and that a lone zero reads as that same
/// default.
///
/// Case: a program written against xterm's control sequence table
/// scrolls a pane back one line with the caret spelling.
#[test]
fn the_xterm_scroll_down_spelling_scrolls_the_region_down() {
    let device = interpret(b"a\x1b[^");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );

    let device = interpret(b"a\x1b[0^");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );
}

/// Asserts that `CSI ^` carrying several parameters still scrolls the
/// region down, rather than being ignored the way a multi-parameter
/// `CSI T` is.
///
/// Case: a program emits the caret spelling with a trailing
/// parameter, such as `CSI 1 ; 2 ^`.
#[test]
fn a_multi_parameter_xterm_scroll_down_spelling_still_scrolls() {
    let device = interpret(b"a\x1b[1;2^");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );
}

/// Asserts that a `CSI T` carrying more than one parameter is ignored
/// rather than read as a scroll down.
///
/// Case: a program starts xterm highlight mouse tracking with
/// `CSI 1;1;1;1;1 T` on a terminal that does not implement it.
#[test]
fn a_multi_parameter_scroll_down_sequence_is_ignored() {
    let device = interpret(b"a\x1b[1;1;1;1;1T");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}

/// Asserts that a delete line raises the chunk liveness.
///
/// Case: an editor's scroll arrives in a chunk that prints nothing.
#[test]
fn a_delete_line_reports_damage() {
    assert!(damage_of(b"\x1b[M"));
}

/// Asserts that Neovim's scroll sequence moves the buffer rows up
/// inside the DECSTBM region, blanks the row that opens at the bottom
/// margin, leaves the tabline and statusline alone, and emits a frame
/// carrying every viewport row.
///
/// Case: Neovim 0.12 on a 51-row window scrolls a file forward by one
/// line with `CSI 3;50 r`, `CSI 3;1 H`, `CSI M`, `CSI r`, and a
/// repaint of the newly exposed bottom row.
#[test]
fn the_neovim_scroll_capture_moves_the_region_and_repaints_every_row() {
    let mut session = Session::sized(GridSize { cols: 4, rows: 51 });
    session.feed(b"\x1b[1;1Ht\x1b[3;1Ha\x1b[4;1Hb\x1b[50;1Hy\x1b[51;1Hz");
    session
        .frame()
        .expect("the setup emits the bootstrap frame");

    let output = session.feed(b"\x1b[3;50r\x1b[3;1H\x1b[M");
    assert!(output.damaged);
    assert_eq!(session.char_at(49, 0), ' ');

    session.feed(b"\x1b[r\x1b[50;1Hn");
    let frame = session.frame().expect("the scroll emits a frame");
    assert_eq!(frame.rows.len(), 51);
    assert_eq!(session.char_at(0, 0), 't');
    assert_eq!(session.char_at(2, 0), 'b');
    assert_eq!(session.char_at(3, 0), ' ');
    assert_eq!(session.char_at(48, 0), 'y');
    assert_eq!(session.char_at(49, 0), 'n');
    assert_eq!(session.char_at(50, 0), 'z');
}
