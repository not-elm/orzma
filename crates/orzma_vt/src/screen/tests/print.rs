//! Tests for graphic character output and the deferred wrap it arms.

use super::*;

/// Asserts that printing stamps the pen into the cell and advances
/// the cursor one column.
///
/// Case: an application prints ordinary colored text at the start
/// of a row.
#[test]
fn print_stamps_the_pen_and_advances() {
    let mut screen = screen();
    screen.pen_mut().fg = Color::Indexed(1);
    let damage = screen.print('a');
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(0)][0].fg, Color::Indexed(1));
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(0), GridColumn(1))
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that printing into the last column arms the deferred
/// wrap and leaves the cursor in place.
///
/// Case: an application emits a line exactly as wide as the
/// screen, and the terminal must not move to the next row until
/// more text actually arrives.
#[test]
fn print_at_the_last_column_arms_the_deferred_wrap() {
    let mut screen = screen();
    screen.state.column = GridColumn(3);
    screen.print('x');
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
}

/// Asserts that the print following an armed deferred wrap lands
/// at the start of the next row and damages that row alone.
///
/// The agreed policy leaves the row the wrap left out of the
/// damage: its contents do not change, and the cursor that moved
/// off it reaches the renderer through the frame's cursor.
///
/// Case: an application prints past the right edge, and the
/// overflowing character continues on the next line.
#[test]
fn the_next_print_after_the_last_column_wraps() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    let damage = screen.print('e');
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'e');
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(1), GridColumn(1))
    );
    assert!(!screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))
    );
}

/// Asserts that damage is reported in viewport rows, not the grid
/// rows the write used.
///
/// Case: the user scrolls back one row and the shell echoes a
/// character at the live tail, which now sits one row lower in the
/// window.
#[test]
fn a_scrolled_screen_reports_damage_in_viewport_rows() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.set_display_offset(DisplayOffset(1));
    screen.state.line = ScreenLine(0);
    assert_eq!(
        screen.print('x'),
        Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))
    );
}

/// Asserts that a deferred wrap on the bottom row scrolls the
/// screen and reports full damage.
///
/// Case: a shell fills the very last cell of the screen and keeps
/// printing, forcing a scroll in the middle of the wrap.
#[test]
fn a_wrap_on_the_bottom_row_scrolls() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.print('x');
    let damage = screen.print('y');
    assert_eq!(screen.grid[ScreenLine(2)][0].c, 'y');
    assert_eq!(damage, Some(DamageSpan::Full));
}

/// Asserts that a write below the bottom of the scrolled window
/// reports no damage at all.
///
/// Case: the user reads scrollback while a build keeps printing at
/// the live tail, which the window no longer shows.
#[test]
fn a_write_scrolled_out_of_the_window_reports_no_damage() {
    let mut screen = screen();
    for _ in 0..3 {
        screen.state.line = ScreenLine(2);
        screen.line_feed();
    }
    screen.viewport.offset = DisplayOffset(3);
    screen.state.line = ScreenLine(0);
    assert_eq!(screen.print('x'), None);
}
