//! Tests for erasing a span of characters at the cursor.

use super::*;
use crate::screen::grid::run::Style;

/// Asserts that erasing characters clears exactly the span at the
/// cursor, leaving the columns before it and the columns after it
/// where they were.
///
/// Case: an application overwrites a two-character field in the
/// middle of a line and clears the old value first.
#[test]
fn erasing_characters_clears_the_span_without_shifting_the_rest() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.grid[ScreenLine(0)][3].c = 'd';
    screen.state.column = GridColumn(1);
    let damage = screen.erase_chars(2, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][2].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'd');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that a count running past the last column stops at the
/// row end rather than reaching the row below, however large it is.
///
/// Case: an application clears the tail of a line by asking for
/// more characters than the row has left.
#[test]
fn erasing_past_the_last_column_stops_at_the_row_end() {
    for count in [10, u16::MAX] {
        let mut screen = screen();
        for c in ['a', 'b', 'c'] {
            screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
        }
        screen.grid[ScreenLine(0)][3].c = 'd';
        screen.grid[ScreenLine(1)][0].c = 'e';
        screen.state.column = GridColumn(2);
        let damage = screen.erase_chars(count, AutoWrap::Enabled);
        assert_eq!(screen.grid[ScreenLine(0)][1].c, 'b');
        assert_eq!(screen.grid[ScreenLine(0)][2].c, ' ');
        assert_eq!(screen.grid[ScreenLine(0)][3].c, ' ');
        assert_eq!(screen.grid[ScreenLine(1)][0].c, 'e');
        assert_eq!(
            damage,
            Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
        );
    }
}

/// Asserts that a cursor outside the scrolling region erases all
/// the same, because ECH ignores the margins.
///
/// Case: an application reserves a status line below its scrolling
/// region and clears a field on it.
#[test]
fn erasing_characters_outside_the_scrolling_region_still_clears() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(0);
    screen.grid[ScreenLine(3)][0].c = 'x';
    screen.grid[ScreenLine(3)][1].c = 'y';
    screen.grid[ScreenLine(3)][2].c = 'z';
    let damage = screen.erase_chars(2, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(3)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(3)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(3)][2].c, 'z');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))
    );
}

/// Asserts that an erase on a row the viewport no longer shows
/// reports no damage while still clearing the cells.
///
/// Case: the user has scrolled back into history when a background
/// program clears a field on the live screen.
#[test]
fn erasing_characters_below_a_scrolled_viewport_reports_no_damage() {
    let mut screen = screen();
    for _ in 0..3 {
        screen.state.line = ScreenLine(2);
        screen.line_feed();
    }
    screen.viewport.offset = DisplayOffset(3);
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(0);
    screen.grid[ScreenLine(0)][0].c = 'a';
    let damage = screen.erase_chars(1, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
    assert_eq!(damage, None);
}

/// Asserts that the cursor stays where it was after an erase,
/// which is what separates ECH from a write.
///
/// Case: an application clears a field and then writes its new
/// value starting from the same position.
#[test]
fn erasing_characters_leaves_the_cursor_on_its_column() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.state.column = GridColumn(1);
    screen.erase_chars(2, AutoWrap::Enabled);
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that erasing characters is a no-op while the deferred
/// wrap is armed, matching `EL 0` rather than erasing the row's
/// last cell.
///
/// Case: an application fills a row to its last column and then
/// issues `CSI 1 X` before printing anything further.
#[test]
fn erasing_characters_is_a_no_op_under_pending_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    let damage = screen.erase_chars(1, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'd');
    assert_eq!(damage, None);
}

/// Asserts that a cursor resting on the last column with no wrap
/// armed erases that column, so the no-op turns on the deferred
/// wrap rather than on the column the cursor sits in.
///
/// Case: an application addresses the last column directly and
/// clears the single character standing there.
#[test]
fn erasing_the_last_column_without_a_pending_wrap_clears_it() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.grid[ScreenLine(0)][3].c = 'd';
    screen.state.column = GridColumn(3);
    let damage = screen.erase_chars(1, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][3].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][2].c, 'c');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}

/// Asserts that erased cells lose the foreground and styling they
/// carried and take the pen's background.
///
/// Case: an application clears a field that was drawn in bold on a
/// colored background, while its pen now carries a different one.
#[test]
fn erasing_characters_clears_the_attributes_and_takes_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().fg = Color::Indexed(1);
    screen.pen_mut().bg = Color::Indexed(2);
    screen.pen_mut().style = Style::BOLD;
    for c in ['a', 'b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.pen_mut().fg = Color::DefaultForeground;
    screen.pen_mut().bg = Color::Indexed(4);
    screen.pen_mut().style = Style::empty();
    screen.state.column = GridColumn(1);
    screen.erase_chars(1, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][1].fg, Color::DefaultForeground);
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::empty());
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(0)][0].fg, Color::Indexed(1));
    assert_eq!(screen.grid[ScreenLine(0)][0].style, Style::BOLD);
}

/// Asserts that erasing characters runs, rather than declining, while a
/// deferred wrap is armed and autowrap is reset.
///
/// Case: a program fills a row, turns autowrap off, and clears the
/// character under the cursor.
#[test]
fn erasing_characters_runs_under_pending_wrap_while_autowrap_is_reset() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert!(screen.state.pending_wrap);
    let damage = screen.erase_chars(1, AutoWrap::Disabled);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', ' ']);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
