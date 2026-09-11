//! Tests for cursor motion and addressing, including the origin mode
//! the addressing is relative to.

use super::*;

/// Asserts that `CSI A` and `CSI B` move the cursor by whole rows
/// and leave it in the column it was already in.
///
/// Case: a full-screen application redraws a column of a table by
/// stepping down it and back up.
#[test]
fn the_cursor_up_and_down_sequences_move_by_rows() {
    let device = interpret(b"\x1b[2;2H\x1b[1Bx\x1b[2Ay");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(2))[1].c, 'x');
    assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, 'y');
}

/// Asserts that `CSI C` and `CSI D` move the cursor by whole
/// columns in the same row.
///
/// Case: a program spaces a label away from the left edge without
/// emitting the blanks between.
#[test]
fn the_cursor_forward_and_back_sequences_move_by_columns() {
    let device = interpret(b"\x1b[2Cx\x1b[2Dy");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, 'x');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'y');
}

/// Asserts that an omitted count moves one row, the default DEC
/// gives every `Pn`.
///
/// Case: a program emits the bare `CSI B` spelling to step down a
/// single row.
#[test]
fn an_omitted_cursor_motion_count_moves_one_row() {
    let device = interpret(b"\x1b[Bx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'x'
    );
}

/// Asserts that `CSI E` moves down and returns to the first
/// column, without scrolling the way `NEL` would.
///
/// Case: a program starts the next record of a listing at the left
/// edge two rows down.
#[test]
fn the_next_line_sequence_moves_down_and_returns_to_column_one() {
    let device = interpret(b"\x1b[1;3Hab\x1b[2Ex");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(2))[0].c, 'x');
}

/// Asserts that `CSI F` moves up and returns to the first column.
///
/// Case: a program rewrites the heading two rows above the row it
/// was filling.
#[test]
fn the_preceding_line_sequence_moves_up_and_returns_to_column_one() {
    let device = interpret(b"\x1b[3;3Hab\x1b[2Fx");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(2))[2].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'x');
}

/// Asserts that `CSI E` at the bottom margin stays put rather than
/// scrolling the region.
///
/// Case: a program emits a next-line at the foot of its pane, where
/// `NEL` would have scrolled but `CNL` must not.
#[test]
fn the_next_line_sequence_does_not_scroll_at_the_bottom() {
    let device = interpret(b"a\x1b[3;1Hb\x1b[Ex");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(2))[0].c, 'x');
}

/// Asserts that `CSI H` addresses the cursor.
///
/// Case: a full-screen application jumps to the second row and
/// second column to draw a box corner.
#[test]
fn the_cursor_position_sequence_addresses_the_cursor() {
    let device = interpret(b"\x1b[2;2Hx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[1].c,
        'x'
    );
}

/// Asserts that `CSI f` addresses the cursor the same way `CSI H`
/// does.
///
/// Case: an older program uses the horizontal-and-vertical-position
/// spelling it was written against.
#[test]
fn the_position_sequence_matches_cursor_position() {
    let device = interpret(b"\x1b[2;2fx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[1].c,
        'x'
    );
}

/// Asserts that origin mode moves the cursor-addressing origin to
/// the top margin.
///
/// Case: an application reserves a header row, turns on origin mode,
/// and addresses the first row of its own pane.
#[test]
fn origin_mode_moves_the_addressing_origin() {
    let device = interpret(b"\x1b[2;3r\x1b[?6h\x1b[1;1Hx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'x'
    );
}

/// Asserts that `CSI ? 6 l` seats the cursor at the upper-left
/// corner.
///
/// Case: a full-screen application drops origin mode on its way out
/// and prints without addressing the cursor first.
#[test]
fn resetting_origin_mode_seats_the_cursor_at_the_corner() {
    let device = interpret(b"\x1b[2;3r\x1b[?6h\x1b[?6lx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'x'
    );
}

/// Asserts that `CSI Pn G` reaches the column-addressing method.
///
/// Case: a full-screen application jumps to column 6 of the row it is
/// already writing and prints there.
#[test]
fn the_cursor_character_absolute_sequence_addresses_a_column() {
    let device = interpret_wide(b"\x1b[6Gx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[5].c,
        'x'
    );
}

/// Asserts that the `HPA` spelling reaches the same column-addressing
/// method as `CHA`.
///
/// Case: an application emits the character-position spelling of the
/// same move and prints there.
#[test]
fn the_character_position_absolute_sequence_addresses_a_column() {
    let device = interpret_wide(b"\x1b[6\x60x");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[5].c,
        'x'
    );
}

/// Asserts that `CSI Pn a` reaches the column-relative motion CUF uses,
/// moving the cursor right by the parameter.
///
/// Case: an application steps two columns right with the
/// character-position-relative spelling and prints there.
#[test]
fn the_character_position_relative_sequence_moves_by_columns() {
    let device = interpret_wide(b"\x1b[2ax");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[2].c,
        'x'
    );
}

/// Asserts that `CSI Pn a` stops at the last column rather than
/// wrapping onto the next row.
///
/// Case: an application asks for a relative move longer than the row
/// and prints where the cursor stopped.
#[test]
fn the_character_position_relative_sequence_stops_at_the_last_column() {
    let device = interpret(b"\x1b[9ax");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[3].c,
        'x'
    );
}

/// Asserts that `CSI Pn d` reaches the line-addressing method rather
/// than moving the cursor down by the parameter.
///
/// Case: an application jumps to row 2 from the home position and
/// prints there.
#[test]
fn the_line_position_absolute_sequence_addresses_a_line() {
    let device = interpret(b"\x1b[2dx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'x'
    );
}
