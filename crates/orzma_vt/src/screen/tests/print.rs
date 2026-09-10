//! Tests for graphic character output and the deferred wrap it arms.

use super::*;
use crate::screen::grid::run::Style;

/// Asserts that a print in insert mode shifts the cells at and right of
/// the cursor one column right, stamps the character at the cursor, and
/// drops the cell pushed past the last column.
///
/// Case: a line editor is in insert mode and the user types a character
/// into the middle of a command that already fills the row.
#[test]
fn an_insert_mode_print_shifts_the_row_right_and_drops_the_last_cell() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'b', 'c']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a print in replace mode overwrites the cell at the
/// cursor and leaves the rest of the row where it was.
///
/// Case: an ordinary shell, which never set IRM, echoes a character over
/// text already on the row.
#[test]
fn a_replace_mode_print_overwrites_the_cell_without_shifting_the_row() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.print('X', InsertReplaceMode::Replace);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'c', 'd']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a print in insert mode at the last column drops the cell
/// it pushes past the border and stamps the character there, arming the
/// deferred wrap as an ordinary print would.
///
/// Case: a program in insert mode types into the rightmost column of a
/// full row.
#[test]
fn an_insert_mode_print_at_the_last_column_replaces_the_cell_it_pushes_out() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'X']);
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a print in insert mode into a row with trailing blanks
/// shifts the text right without losing any visible cell.
///
/// Case: a program in insert mode types into a half-filled row.
#[test]
fn an_insert_mode_print_into_a_padded_row_loses_no_visible_cell() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b']);
    screen.state.column = GridColumn(1);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'b', ' ']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that an armed deferred wrap is resolved before the insert
/// shift, so the shift lands on the row the wrap moved to and the row it
/// left is untouched.
///
/// Case: a program in insert mode fills a row exactly and then types one
/// more character, which continues on the next line.
#[test]
fn an_armed_deferred_wrap_resolves_before_the_insert_shifts_the_new_row() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace);
    }
    seed_row(&mut screen, ScreenLine(1), &['p', 'q', 'r', 's']);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'd']);
    assert_eq!(row_glyphs(&screen, ScreenLine(1)), vec!['X', 'p', 'q', 'r']);
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(1), GridColumn(1))
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))
    );
}

/// Asserts that an insert-mode print stamps the whole current pen at the
/// cursor while each cell the shift moves keeps the attributes it was
/// printed with.
///
/// Case: a TUI in insert mode types a character into a row it had drawn
/// one cell at a time in different colors.
#[test]
fn an_insert_mode_print_keeps_the_shifted_cells_attributes() {
    let mut screen = screen();
    for (c, bg) in [('a', 1), ('b', 2), ('c', 3), ('d', 5)] {
        screen.pen_mut().bg = Color::Indexed(bg);
        screen.print(c, InsertReplaceMode::Replace);
    }
    screen.pen_mut().style = Style::ITALIC;
    screen.pen_mut().fg = Color::Indexed(6);
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.state.pending_wrap = false;
    screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, 'X');
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::ITALIC);
    assert_eq!(screen.grid[ScreenLine(0)][1].fg, Color::Indexed(6));
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(0)][2].c, 'b');
    assert_eq!(screen.grid[ScreenLine(0)][2].bg, Color::Indexed(2));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'c');
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(3));
}

/// Asserts that an insert-mode print on a one-column screen replaces the
/// only cell, because the character it shifts falls past the border.
///
/// Case: the window is dragged down to a single column while a program
/// in insert mode keeps printing.
#[test]
fn an_insert_mode_print_on_a_single_column_screen_replaces_the_only_cell() {
    let mut screen = Screen::new(GridSize { cols: 1, rows: 1 }, 10);
    seed_row(&mut screen, ScreenLine(0), &['a']);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['X']);
    assert!(screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that an insert-mode print whose deferred wrap scrolls the
/// screen reports full damage rather than the shifted row alone.
///
/// Case: a program in insert mode fills the very last cell of the screen
/// and keeps printing, forcing a scroll in the middle of the wrap.
#[test]
fn an_insert_mode_print_that_scrolls_on_wrap_reports_full_damage() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.print('x', InsertReplaceMode::Insert);
    let damage = screen.print('y', InsertReplaceMode::Insert);
    assert_eq!(screen.grid[ScreenLine(2)][0].c, 'y');
    assert_eq!(damage, Some(DamageSpan::Full));
}

/// Asserts that printing stamps the pen into the cell and advances
/// the cursor one column.
///
/// Case: an application prints ordinary colored text at the start
/// of a row.
#[test]
fn print_stamps_the_pen_and_advances() {
    let mut screen = screen();
    screen.pen_mut().fg = Color::Indexed(1);
    let damage = screen.print('a', InsertReplaceMode::Replace);
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
    screen.print('x', InsertReplaceMode::Replace);
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
        screen.print(c, InsertReplaceMode::Replace);
    }
    let damage = screen.print('e', InsertReplaceMode::Replace);
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
        screen.print('x', InsertReplaceMode::Replace),
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
    screen.print('x', InsertReplaceMode::Replace);
    let damage = screen.print('y', InsertReplaceMode::Replace);
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
    assert_eq!(screen.print('x', InsertReplaceMode::Replace), None);
}
