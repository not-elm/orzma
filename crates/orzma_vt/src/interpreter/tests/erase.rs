//! Tests for erasure in the display and in the line.

use super::*;

/// Asserts that `CSI J` erases from the cursor to the end of the
/// screen and leaves what precedes it.
///
/// Case: a program finishes drawing a short menu and clears the
/// stale rows a longer one left below it.
#[test]
fn the_erase_in_display_sequence_clears_below_the_cursor() {
    let device = interpret(b"ab\x1b[2;1Hcd\x1b[2;2H\x1b[J");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, 'c');
    assert_eq!(screen.viewport_row(ViewportLine(1))[1].c, ' ');
}

/// Asserts that `CSI 2 J` clears the whole visible screen.
///
/// Case: a full-screen application takes over and wipes whatever
/// the shell left behind before its first paint.
#[test]
fn the_erase_in_display_sequence_clears_the_whole_screen() {
    let device = interpret(b"ab\x1b[2;1Hcd\x1b[2J");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, ' ');
}

/// Asserts that an `ED` parameter this terminal does not answer
/// leaves the screen alone rather than erasing something.
///
/// Case: an application asks for `ED 3` to drop the scrollback,
/// which this terminal does not model.
#[test]
fn an_unanswered_erase_in_display_parameter_erases_nothing() {
    let device = interpret(b"ab\x1b[3J");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}

/// Asserts that `CSI K` erases from the cursor to the end of the
/// row and leaves the rows around it.
///
/// Case: a shell redraws a prompt line after the user deletes the
/// tail of what they typed.
#[test]
fn the_erase_in_line_sequence_clears_to_the_end_of_the_row() {
    let device = interpret(b"abc\x1b[1;2H\x1b[K");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, ' ');
}

/// Asserts that `CSI 2 K` clears the whole row the cursor sits on.
///
/// Case: a status line is rewritten from scratch each time its
/// contents change.
#[test]
fn the_erase_in_line_sequence_clears_the_whole_row() {
    let device = interpret(b"abc\x1b[1;2H\x1b[2K");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        ' '
    );
}

/// Asserts that `CSI Pn X` reaches the screen and erases `Pn` cells
/// from the cursor without moving it.
///
/// Case: Neovim clears the padding of a file-tree pane, which does
/// not reach the right edge of the screen.
#[test]
fn the_erase_character_sequence_clears_the_span_at_the_cursor() {
    let device = interpret(b"abcd\x1b[1;2H\x1b[2X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[3].c, 'd');
    assert_eq!(screen.cursor_column(), GridColumn(1));
}

/// Asserts that an erase character raises the chunk liveness, so the
/// row it cleared is repainted.
///
/// Case: Neovim clears the padding of a file-tree pane in a chunk
/// that prints nothing of its own, having selected the background it
/// wants the cleared cells to carry.
#[test]
fn an_erase_character_reports_damage() {
    assert!(damage_of(b"\x1b[41m\x1b[2X"));
}

/// Asserts that an omitted, zero, or explicit one erase count all
/// erase a single cell.
///
/// Case: a program emits the terminfo `ech` capability, whose
/// parameter it leaves at the default.
#[test]
fn an_omitted_or_zero_erase_character_count_erases_one_cell() {
    let device = interpret(b"abc\x1b[1;1H\x1b[X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'b');

    let device = interpret(b"abc\x1b[1;1H\x1b[0X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'b');

    let device = interpret(b"abc\x1b[1;1H\x1b[1X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'b');
}

/// Asserts that `CSI K` still erases to the end of the line after a
/// `CSI Z` has stepped the cursor back out of a filled row.
///
/// Case: a form-filling program writes into the last column, steps back
/// to the previous tab stop with Shift-Tab, and clears the rest of the
/// line before rewriting the field.
#[test]
fn an_erase_to_end_runs_after_a_backward_tabulation_out_of_a_full_row() {
    let device = interpret_wide(b"\x1b[1;20HX\x1b[Z\x1b[K");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[16].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[19].c, ' ');
}
