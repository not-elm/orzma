//! Tests for deleting characters at the cursor within one row.

use super::*;
use crate::screen::grid::run::Style;

/// Asserts that a delete closes the gap by moving the cells right of the
/// cursor left, and blanks the column that opens at the right margin,
/// leaving the cursor where it was.
///
/// Case: a shell's line editor deletes a character from the middle of a
/// command that fills the row.
#[test]
fn a_delete_closes_the_gap_and_blanks_the_right_margin() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'c', 'd', blank]
    );
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a delete carries each shifted cell's attributes with it
/// while the blank opening at the right margin takes the pen background
/// and none of the pen's rendition.
///
/// Case: a TUI drawing on a colored background deletes a character from
/// text it had already drawn bold.
#[test]
fn the_shifted_cells_keep_their_attributes_and_the_new_blank_takes_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().style = Style::BOLD;
    screen.pen_mut().bg = Color::Indexed(1);
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.delete_characters(1);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, 'c');
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::BOLD);
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(1));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][3].style, Style::empty());
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(4));
}

/// Asserts that a count above one deletes that many characters in a
/// single call and blanks one column per deleted character.
///
/// Case: an editor removes a two-column indent guide from the start of a
/// row it is repainting.
#[test]
fn a_count_above_one_deletes_that_many_and_blanks_as_many_columns() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(0);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(2);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['c', 'd', blank, blank]
    );
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a count larger than the characters left in the row
/// deletes only those and blanks the columns they vacated.
///
/// Case: an application asks to delete more columns than the row has
/// left while the cursor sits near the right edge.
#[test]
fn a_count_past_the_row_deletes_only_the_remaining_characters() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(2);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(9);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'b', blank, blank]
    );
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a delete in the last column blanks that one cell and
/// leaves every column before it untouched.
///
/// Case: the cursor rests on the final column of a full row when the
/// application deletes the character under it.
#[test]
fn a_delete_in_the_last_column_blanks_that_cell_alone() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'b', 'c', blank]
    );
    assert_eq!(screen.state.column, GridColumn(3));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a delete applies on the cursor's row even when that row
/// lies outside the scroll region, following xterm rather than VT510's
/// "no effect outside the scrolling margins".
///
/// Case: a full-screen application parks its cursor on a status row
/// below the region it scrolls and edits that row in place.
#[test]
fn a_delete_outside_the_scroll_region_still_closes_the_gap() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    seed_row(&mut screen, ScreenLine(3), &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(3)),
        vec!['a', 'c', 'd', blank]
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))
    );
}

/// Asserts that a zero count deletes nothing and reports no damage, while
/// still disarming the deferred wrap.
///
/// Case: a caller inside the crate passes a count it computed as zero.
#[test]
fn a_zero_count_deletes_nothing() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    screen.state.pending_wrap = true;
    let damage = screen.delete_characters(0);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'd']);
    assert_eq!(damage, None);
    assert!(!screen.state.pending_wrap);
}

/// Asserts that a delete disarms the deferred wrap, so the next printed
/// character stays on the cursor's row.
///
/// Case: an application fills the last column of a row and then deletes
/// a character before printing again.
#[test]
fn a_delete_disarms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    assert!(screen.state.pending_wrap);
    screen.delete_characters(1);
    assert!(!screen.state.pending_wrap);
    screen.print('x');
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
}

/// Asserts that a delete on a row the viewport has scrolled past still
/// closes the gap while reporting no damage.
///
/// Case: the user is reading scrollback when a program edits a line on
/// the live screen below the view.
#[test]
fn a_delete_below_a_scrolled_viewport_reports_no_damage() {
    let mut screen = screen();
    for _ in 0..3 {
        screen.state.line = ScreenLine(2);
        screen.line_feed();
    }
    screen.viewport.offset = DisplayOffset(3);
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'c', 'd', blank]
    );
    assert_eq!(damage, None);
}
