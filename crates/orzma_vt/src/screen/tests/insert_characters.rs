//! Tests for inserting blank characters at the cursor within one row.

use super::*;
use crate::screen::grid::run::Style;

/// Asserts that an insert opens one blank at the cursor, moves the
/// cells right of it one column right, and drops the cell pushed past
/// the last column, leaving the cursor where it was.
///
/// Case: a shell's line editor inserts a character into the middle of a
/// command that already fills the row.
#[test]
fn an_insert_opens_a_blank_at_the_cursor_and_drops_the_last_cell() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', blank, 'b', 'c']
    );
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that the blank an insert opens carries the pen background
/// with none of the pen's rendition, while the cells it shifts keep the
/// attributes they were printed with.
///
/// Case: a TUI drawing on a colored background inserts a character into
/// text it had already drawn bold.
#[test]
fn an_inserted_blank_carries_the_pen_background_without_its_rendition() {
    let mut screen = screen();
    screen.pen_mut().style = Style::BOLD;
    screen.pen_mut().bg = Color::Indexed(1);
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.insert_characters(1);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::empty());
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(0)][2].c, 'b');
    assert_eq!(screen.grid[ScreenLine(0)][2].style, Style::BOLD);
    assert_eq!(screen.grid[ScreenLine(0)][2].bg, Color::Indexed(1));
}

/// Asserts that a count above one opens that many blanks in a single
/// call.
///
/// Case: an editor makes room for a two-column indent guide at the
/// start of a row it is repainting.
#[test]
fn a_count_above_one_opens_that_many_blanks() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(0);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(2);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec![blank, blank, 'a', 'b']
    );
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a count past the columns left in the row blanks every
/// cell from the cursor to the right edge rather than shifting past it.
///
/// Case: an application asks to insert more columns than the row has
/// left while the cursor sits near the right edge.
#[test]
fn a_count_past_the_row_blanks_the_rest_of_the_row() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(2);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(9);
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

/// Asserts that an insert in the last column replaces that one cell and
/// leaves every column before it untouched.
///
/// Case: the cursor rests on the final column of a full row when the
/// application inserts a character there.
#[test]
fn an_insert_in_the_last_column_replaces_that_cell_alone() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
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

/// Asserts that an insert applies on the cursor's row even when that row
/// lies outside the scroll region.
///
/// Case: a full-screen application parks its cursor on a status row
/// below the region it scrolls and edits that row in place.
#[test]
fn an_insert_outside_the_scroll_region_still_shifts_the_row() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    seed_row(&mut screen, ScreenLine(3), &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(3)),
        vec!['a', blank, 'b', 'c']
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))
    );
}

/// Asserts that a zero count inserts nothing and reports no damage.
///
/// Case: a caller inside the crate passes a count it computed as zero.
#[test]
fn a_zero_count_inserts_nothing() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.insert_characters(0);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'd']);
    assert_eq!(damage, None);
}

/// Asserts that an insert disarms the deferred wrap, so the next printed
/// character stays on the cursor's row.
///
/// Case: an application fills the last column of a row and then inserts
/// a character before printing again.
#[test]
fn an_insert_disarms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert!(screen.state.pending_wrap);
    screen.insert_characters(1);
    assert!(!screen.state.pending_wrap);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
}

/// Asserts that an insert on a row the viewport has scrolled past still
/// shifts the row while reporting no damage.
///
/// Case: the user is reading scrollback when a program edits a line on
/// the live screen below the view.
#[test]
fn an_insert_below_a_scrolled_viewport_reports_no_damage() {
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
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', blank, 'b', 'c']
    );
    assert_eq!(damage, None);
}
