//! Tests for scrolling the whole region up without moving the cursor.

use super::*;

/// Asserts that a scroll up rotates the whole region, blanks the
/// bottom-margin row, leaves rows outside the region alone, and leaves
/// the cursor exactly where it was.
///
/// Case: a program with a pinned header scrolls the pane below it
/// forward while its cursor rests mid-row on a line it is editing.
#[test]
fn a_scroll_up_rotates_the_region_and_leaves_the_cursor_alone() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.scroll_region_up(1);
    assert_eq!(glyphs(&screen, 4), vec!['a', 'c', 'd', blank]);
    assert_eq!(damage, Some(DamageSpan::Full));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
    assert_eq!(screen.grid.history_len(), 0);
}

/// Asserts that a scroll up moves the region even when the cursor
/// sits outside it, and that a top margin on the first row feeds the
/// departing row to history even though a bottom margin pins content
/// below the region.
///
/// Case: a program parks its cursor on a status row below the region
/// and scrolls the region from there.
#[test]
fn a_scroll_up_ignores_where_the_cursor_is() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(2),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(3);
    let blank = screen.state.pen.erase_cell().c;
    screen.scroll_region_up(1);
    assert_eq!(glyphs(&screen, 4), vec!['b', 'c', blank, 'd']);
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.grid.row(GridLine(-1))[0].c, 'a');
}

/// Asserts that a count larger than the region blanks the whole
/// region and nothing more.
///
/// Case: an application asks to scroll by more lines than its region
/// holds.
#[test]
fn a_count_past_the_region_height_scrolls_only_the_region() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    let blank = screen.state.pen.erase_cell().c;
    screen.scroll_region_up(9);
    assert_eq!(glyphs(&screen, 4), vec!['a', blank, blank, 'd']);
}

/// Asserts that a scroll up of a region whose top margin is the first
/// row feeds every departing row to history and walks a scrolled-back
/// viewport one row further per departing row.
///
/// Case: the user is reading scrollback while a program scrolls the
/// full page up by two lines.
#[test]
fn a_scroll_up_from_the_first_row_feeds_history_and_holds_the_viewport_per_row() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.set_display_offset(DisplayOffset(1));
    seed(&mut screen, &['a', 'b', 'c']);
    screen.scroll_region_up(2);
    assert_eq!(screen.grid.history_len(), 3);
    assert_eq!(screen.grid.row(GridLine(-2))[0].c, 'a');
    assert_eq!(screen.grid.row(GridLine(-1))[0].c, 'b');
    assert_eq!(screen.display_offset(), DisplayOffset(3));
}
