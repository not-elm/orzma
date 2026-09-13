//! Tests for graphic character output and the deferred wrap it arms.

use super::*;
use crate::screen::cell::{CellExtra, CellWidth, MAX_COMBINING};
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
    let damage = screen.print('X', InsertReplaceMode::Insert, AutoWrap::Enabled);
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
    let damage = screen.print('X', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'c', 'd']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a print in insert mode at the last column drops the cell
/// it pushes past the border and stamps the character there, arming the
/// deferred wrap.
///
/// Case: a program in insert mode types into the rightmost column of a
/// full row.
#[test]
fn an_insert_mode_print_at_the_last_column_replaces_the_cell_it_pushes_out() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let damage = screen.print('X', InsertReplaceMode::Insert, AutoWrap::Enabled);
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
    let damage = screen.print('X', InsertReplaceMode::Insert, AutoWrap::Enabled);
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
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    seed_row(&mut screen, ScreenLine(1), &['p', 'q', 'r', 's']);
    let damage = screen.print('X', InsertReplaceMode::Insert, AutoWrap::Enabled);
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
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.pen_mut().style = Style::ITALIC;
    screen.pen_mut().fg = Color::Indexed(6);
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.state.pending_wrap = false;
    screen.print('X', InsertReplaceMode::Insert, AutoWrap::Enabled);
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
/// only cell.
///
/// Case: the window is dragged down to a single column while a program
/// in insert mode keeps printing.
#[test]
fn an_insert_mode_print_on_a_single_column_screen_replaces_the_only_cell() {
    let mut screen = Screen::new(GridSize { cols: 1, rows: 1 }, 10);
    seed_row(&mut screen, ScreenLine(0), &['a']);
    let damage = screen.print('X', InsertReplaceMode::Insert, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['X']);
    assert!(screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
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
    let damage = screen.print('a', InsertReplaceMode::Replace, AutoWrap::Enabled);
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
/// screen, and no further text has arrived yet.
#[test]
fn print_at_the_last_column_arms_the_deferred_wrap() {
    let mut screen = screen();
    screen.state.column = GridColumn(3);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
}

/// Asserts that the print following an armed deferred wrap lands at the
/// start of the next row and damages that row alone, leaving out the row
/// the wrap left.
///
/// Case: an application prints past the right edge, and the overflowing
/// character continues on the next line.
#[test]
fn the_next_print_after_the_last_column_wraps() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    let damage = screen.print('e', InsertReplaceMode::Replace, AutoWrap::Enabled);
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
        screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled),
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
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    let damage = screen.print('y', InsertReplaceMode::Replace, AutoWrap::Enabled);
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
    assert_eq!(
        screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled),
        None
    );
}

/// Asserts that a print into the last column with autowrap reset
/// replaces the cell in place and leaves the deferred wrap disarmed.
///
/// Case: a status-bar program turns autowrap off and draws text that
/// reaches the rightmost column of the row.
#[test]
fn a_print_at_the_last_column_without_autowrap_leaves_the_wrap_disarmed() {
    let mut screen = screen();
    screen.state.column = GridColumn(3);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Disabled);
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that successive prints at the right border with autowrap
/// reset keep replacing the last column instead of moving to the next
/// row.
///
/// Case: a program with autowrap off writes a line longer than the
/// screen is wide.
#[test]
fn prints_past_the_right_border_without_autowrap_replace_the_last_column() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd', 'e', 'f'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Disabled);
    }
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'f']);
    assert_eq!(row_glyphs(&screen, ScreenLine(1)), vec![' ', ' ', ' ', ' ']);
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that a deferred wrap armed while autowrap was set does not
/// fire once autowrap is reset, so the character replaces the last
/// column rather than reaching the next row.
///
/// Case: an application fills a row and then turns autowrap off before
/// the next character arrives.
#[test]
fn an_armed_wrap_does_not_fire_once_autowrap_is_reset() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert!(screen.state.pending_wrap);
    screen.print('e', InsertReplaceMode::Replace, AutoWrap::Disabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'e']);
    assert_eq!(row_glyphs(&screen, ScreenLine(1)), vec![' ', ' ', ' ', ' ']);
    assert!(!screen.state.pending_wrap);
}

/// Asserts that the first print after autowrap returns replaces the
/// last column and arms the wrap, so only the print after that one
/// reaches the next row.
///
/// Case: a program turns autowrap off, writes to the right border, and
/// turns it back on before printing again.
#[test]
fn the_first_print_after_autowrap_returns_replaces_and_then_arms() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Disabled);
    }
    assert!(!screen.state.pending_wrap);
    screen.print('e', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'e']);
    assert!(screen.state.pending_wrap);
    screen.print('f', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'f');
}

/// Asserts that a print away from the last column disarms a deferred
/// wrap the cursor carried in from the right border.
///
/// Case: a backward tabulation moves the cursor off the right border of
/// a filled row without disarming the flag, and the application then
/// prints mid-row while autowrap is reset.
#[test]
fn a_print_away_from_the_last_column_disarms_a_leftover_wrap() {
    let mut screen = screen();
    screen.state.pending_wrap = true;
    screen.state.column = GridColumn(1);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Disabled);
    assert_eq!(screen.state.column, GridColumn(2));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that an insert-mode print at the last column with autowrap
/// reset replaces the cell in place, leaves the row unshifted, keeps
/// the wrap disarmed, and carries the printing pen's own foreground and
/// background.
///
/// Case: a program in insert mode with autowrap off types into the
/// rightmost column while a non-default foreground and background
/// colour are both selected.
#[test]
fn an_insert_mode_print_at_the_last_column_without_autowrap_replaces_in_place() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    screen.pen_mut().fg = Color::Indexed(2);
    screen.pen_mut().bg = Color::Indexed(4);
    screen.print('X', InsertReplaceMode::Insert, AutoWrap::Disabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'X']);
    assert_eq!(screen.grid[ScreenLine(0)][3].fg, Color::Indexed(2));
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(4));
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that the last-column predicate answers for the cursor's
/// position alone, independent of any glyph width.
///
/// Case: erase-to-end-of-line asks whether the cursor is parked past the
/// row while the cursor sits at the right margin.
#[test]
fn the_last_column_predicate_ignores_width() {
    let mut screen = Screen::new(GridSize { cols: 4, rows: 1 }, 10);
    assert!(!screen.is_last_column());
    screen.state.column = GridColumn(3);
    assert!(screen.is_last_column());
}

/// Asserts that a width-2 glyph does not fit when only one column
/// remains, while a width-1 glyph fits on the last column.
///
/// Case: a fullwidth character arrives with the cursor one column short
/// of the right margin.
#[test]
fn a_wide_glyph_needs_two_remaining_columns() {
    let mut screen = Screen::new(GridSize { cols: 4, rows: 1 }, 10);
    screen.state.column = GridColumn(3);
    assert!(screen.fits(1));
    assert!(!screen.fits(2));
    screen.state.column = GridColumn(2);
    assert!(screen.fits(2));
}

/// Every cell width of one row, left to right.
fn row_widths(screen: &Screen, line: ScreenLine) -> Vec<CellWidth> {
    (0..screen.grid.size().cols)
        .map(|column| screen.grid[line][column].width)
        .collect()
}

/// Asserts that a fullwidth glyph occupies its column and the next as a
/// body and a continuation, and advances the cursor by two.
///
/// Case: a shell echoes a Japanese character at the start of a row.
#[test]
fn a_wide_glyph_takes_two_columns_and_advances_by_two() {
    let mut screen = screen();
    let damage = screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['あ', ' ', ' ', ' ']
    );
    assert_eq!(
        row_widths(&screen, ScreenLine(0)),
        vec![
            CellWidth::Wide,
            CellWidth::Spacer,
            CellWidth::Narrow,
            CellWidth::Narrow
        ]
    );
    assert_eq!(screen.state.column, GridColumn(2));
    assert!(!screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a fullwidth glyph landing on the last two columns parks
/// the cursor on the last column and arms the deferred wrap.
///
/// Case: a row of four columns receives `abあ` followed by `い`.
#[test]
fn a_wide_glyph_ending_the_row_arms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'あ'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'b', 'あ', ' ']
    );
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
    screen.print('い', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(1)),
        vec!['い', ' ', ' ', ' ']
    );
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a fullwidth glyph with one column left wraps to the next
/// row and leaves a leading spacer carrying the live pen in the last
/// column, reporting both rows as damaged.
///
/// Case: a program with a colored background prints `abcあ` on a
/// four-column row.
#[test]
fn a_wide_glyph_with_one_column_left_wraps_and_leaves_a_filler() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(4);
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    let damage = screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', ' ']);
    assert_eq!(
        screen.grid[ScreenLine(0)][3].width,
        CellWidth::LeadingSpacer
    );
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(4));
    assert_eq!(
        row_glyphs(&screen, ScreenLine(1)),
        vec!['あ', ' ', ' ', ' ']
    );
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(2));
    assert!(!screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(1)))
    );
}

/// Asserts that a fullwidth glyph wrapping off the bottom row scrolls
/// and reports the whole viewport as damaged.
///
/// Case: a Japanese log line reaches the right edge of the last row.
#[test]
fn a_wide_glyph_wrapping_on_the_bottom_row_scrolls() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    let damage = screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(damage, Some(DamageSpan::Full));
    assert_eq!(
        row_glyphs(&screen, ScreenLine(2)),
        vec!['あ', ' ', ' ', ' ']
    );
    assert_eq!(
        screen.grid[ScreenLine(1)][3].width,
        CellWidth::LeadingSpacer
    );
}

/// Asserts that on a two-column screen a fullwidth glyph fills the row,
/// parks the cursor on the last column, and the next one wraps.
///
/// Case: a pane at the minimum width receives two Japanese characters.
#[test]
fn a_wide_glyph_on_a_two_column_screen_fills_the_row() {
    let mut screen = Screen::new(GridSize { cols: 2, rows: 3 }, 10);
    screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.state.column, GridColumn(1));
    assert!(screen.state.pending_wrap);
    screen.print('い', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['あ', ' ']);
    assert_eq!(row_glyphs(&screen, ScreenLine(1)), vec!['い', ' ']);
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that a fullwidth glyph that does not fit with autowrap reset
/// is dropped, leaves the row untouched, and disarms the deferred wrap
/// rather than arming it.
///
/// Case: a status-bar program with autowrap off draws a Japanese label
/// that runs past the right edge.
#[test]
fn a_wide_glyph_that_does_not_fit_without_autowrap_is_dropped() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Disabled);
    }
    let damage = screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Disabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', ' ']);
    assert_eq!(screen.grid[ScreenLine(0)][3].width, CellWidth::Narrow);
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(damage, None);
}

/// Asserts that a deferred wrap a restore brought back does not survive
/// a dropped fullwidth glyph, so the next print after autowrap returns
/// does not wrap.
///
/// Case: a program restores a cursor saved at the right edge, turns
/// autowrap off, prints a Japanese character, and turns autowrap on.
#[test]
fn a_dropped_wide_glyph_clears_a_restored_deferred_wrap() {
    let mut screen = screen();
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Disabled);
    assert!(!screen.state.pending_wrap);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that a narrow glyph printed over a wide body blanks the
/// continuation column it leaves behind.
///
/// Case: a program overwrites the left half of a Japanese character with
/// an ASCII letter.
#[test]
fn a_narrow_glyph_over_a_wide_body_blanks_its_continuation() {
    let mut screen = screen();
    screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    screen.state.column = GridColumn(0);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['x', ' ', ' ', ' ']);
    assert_eq!(screen.grid[ScreenLine(0)][1].width, CellWidth::Narrow);
}

/// Asserts that a narrow glyph printed over a continuation column blanks
/// the wide body to its left.
///
/// Case: a program overwrites the right half of a Japanese character.
#[test]
fn a_narrow_glyph_over_a_continuation_blanks_its_body() {
    let mut screen = screen();
    screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    screen.state.column = GridColumn(1);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec![' ', 'x', ' ', ' ']);
    assert_eq!(screen.grid[ScreenLine(0)][0].width, CellWidth::Narrow);
}

/// Asserts that a wide glyph landing on the second half of one pair and
/// the first half of another blanks both damaged neighbours.
///
/// Case: a program overwrites the middle of `あい` with `う` shifted one
/// column right.
#[test]
fn a_wide_glyph_over_two_half_pairs_blanks_both_neighbours() {
    let mut screen = screen();
    for c in ['あ', 'い'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.state.column = GridColumn(1);
    screen.state.pending_wrap = false;
    screen.print('う', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec![' ', 'う', ' ', ' ']
    );
    assert_eq!(
        row_widths(&screen, ScreenLine(0)),
        vec![
            CellWidth::Narrow,
            CellWidth::Wide,
            CellWidth::Spacer,
            CellWidth::Narrow
        ]
    );
}

/// Asserts that a wide glyph in insert mode shifts the row right by two
/// columns before landing.
///
/// Case: a line editor in insert mode receives a Japanese character in
/// the middle of `abcd`.
#[test]
fn an_insert_mode_wide_glyph_shifts_the_row_by_two() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    screen.print('あ', InsertReplaceMode::Insert, AutoWrap::Enabled);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'あ', ' ', 'b']
    );
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that a wide glyph in insert mode landing on the last two
/// columns overwrites them in place without shifting.
///
/// Case: a line editor in insert mode receives a Japanese character two
/// columns from the right edge.
#[test]
fn an_insert_mode_wide_glyph_ending_the_row_overwrites_in_place() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(2);
    screen.print('あ', InsertReplaceMode::Insert, AutoWrap::Enabled);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'b', 'あ', ' ']
    );
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
}

/// Asserts that a combining mark joins the cell the cursor just passed,
/// leaves the cursor where it is, and reports that row as damaged.
///
/// Case: a shell echoes `e` followed by U+0301 COMBINING ACUTE ACCENT.
#[test]
fn a_combining_mark_joins_the_previous_cell_and_reports_damage() {
    let mut screen = screen();
    screen.print('e', InsertReplaceMode::Replace, AutoWrap::Enabled);
    let damage = screen.print('\u{0301}', InsertReplaceMode::Replace, AutoWrap::Enabled);
    let cell = &screen.grid[ScreenLine(0)][0];
    assert_eq!(cell.c, 'e');
    assert_eq!(
        cell.extra.as_deref().map(CellExtra::marks),
        Some(&['\u{0301}'][..])
    );
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a combining mark arriving while the deferred wrap is
/// armed joins the last column's cell and does not resolve the wrap.
///
/// Case: a row ends in `e` and the accent arrives after the cursor
/// parked on the right edge.
#[test]
fn a_combining_mark_under_an_armed_wrap_joins_the_last_cell() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'e'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert!(screen.state.pending_wrap);
    screen.print('\u{0301}', InsertReplaceMode::Replace, AutoWrap::Enabled);
    let cell = &screen.grid[ScreenLine(0)][3];
    assert_eq!(
        cell.extra.as_deref().map(CellExtra::marks),
        Some(&['\u{0301}'][..])
    );
    assert_eq!(screen.state.line, ScreenLine(0));
    assert!(screen.state.pending_wrap);
}

/// Asserts that with autowrap reset a combining mark at the last column
/// joins the cell under the cursor rather than the one to its left.
///
/// Case: a program with autowrap off prints `e` in the last column and
/// then its accent.
#[test]
fn a_combining_mark_without_autowrap_joins_the_cell_under_the_cursor() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'e'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Disabled);
    }
    screen.print('\u{0301}', InsertReplaceMode::Replace, AutoWrap::Disabled);
    assert!(screen.grid[ScreenLine(0)][2].extra.is_none());
    assert_eq!(
        screen.grid[ScreenLine(0)][3]
            .extra
            .as_deref()
            .map(CellExtra::marks),
        Some(&['\u{0301}'][..])
    );
}

/// Asserts that a combining mark after a fullwidth glyph joins the wide
/// body rather than its continuation column.
///
/// Case: a program prints a Japanese character followed by U+3099
/// COMBINING KATAKANA-HIRAGANA VOICED SOUND MARK.
#[test]
fn a_combining_mark_after_a_wide_glyph_joins_its_body() {
    let mut screen = screen();
    screen.print('か', InsertReplaceMode::Replace, AutoWrap::Enabled);
    screen.print('\u{3099}', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(
        screen.grid[ScreenLine(0)][0]
            .extra
            .as_deref()
            .map(CellExtra::marks),
        Some(&['\u{3099}'][..])
    );
    assert!(screen.grid[ScreenLine(0)][1].extra.is_none());
}

/// Asserts that a combining mark at the start of a row is kept on the
/// first cell rather than dropped.
///
/// Case: a stream is cut between a base character and its accent, and
/// the accent arrives on a fresh row.
#[test]
fn a_combining_mark_at_the_row_start_is_kept_on_the_first_cell() {
    let mut screen = screen();
    let damage = screen.print('\u{0301}', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(
        screen.grid[ScreenLine(0)][0]
            .extra
            .as_deref()
            .map(CellExtra::marks),
        Some(&['\u{0301}'][..])
    );
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a mark past the per-cell cap is dropped and reports no
/// damage.
///
/// Case: a stream piles combining marks onto one cell far past any
/// typographic need.
#[test]
fn a_combining_mark_past_the_cap_is_dropped_without_damage() {
    let mut screen = screen();
    screen.print('e', InsertReplaceMode::Replace, AutoWrap::Enabled);
    for _ in 0..MAX_COMBINING {
        assert!(
            screen
                .print('\u{0301}', InsertReplaceMode::Replace, AutoWrap::Enabled)
                .is_some()
        );
    }
    let damage = screen.print('\u{0302}', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(damage, None);
    let marks = screen.grid[ScreenLine(0)][0]
        .extra
        .as_deref()
        .map(CellExtra::marks);
    assert_eq!(marks.map(<[char]>::len), Some(MAX_COMBINING));
}

/// Asserts that a control character reaching the printer is ignored
/// without moving the cursor or reporting damage.
///
/// Case: a raw NUL slips through to the printer.
#[test]
fn a_control_character_is_ignored_by_the_printer() {
    let mut screen = screen();
    let damage = screen.print('\0', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(damage, None);
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec![' ', ' ', ' ', ' ']);
}

/// Asserts that a fullwidth glyph on a one-column screen is dropped
/// rather than stamped past the row.
///
/// Case: a test-built one-column screen receives a Japanese character.
#[test]
fn a_wide_glyph_on_a_one_column_screen_is_dropped() {
    let mut screen = Screen::new(GridSize { cols: 1, rows: 1 }, 10);
    let damage = screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(damage, None);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that a combining mark whose candidate cell is a wrap filler
/// is dropped without damage.
///
/// Case: a Japanese character wrapped at the right edge, and the
/// application then moves the cursor back onto that row's last column
/// before an accent arrives.
#[test]
fn a_combining_mark_on_a_wrap_filler_is_dropped() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'あ'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert_eq!(
        screen.grid[ScreenLine(0)][3].width,
        CellWidth::LeadingSpacer
    );
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = false;
    let damage = screen.print('\u{0301}', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(damage, None);
    assert!(screen.grid[ScreenLine(0)][3].extra.is_none());
    assert_eq!(
        screen.grid[ScreenLine(0)][3].width,
        CellWidth::LeadingSpacer
    );
}
