//! Tests for scrolling the whole region down without moving the
//! cursor.

use super::*;

/// Asserts that a scroll down rotates the whole region, blanks the
/// top-margin row, discards the bottom-margin row, leaves rows outside
/// the region alone, and leaves the cursor exactly where it was.
///
/// Case: a program with a pinned header scrolls the pane below it
/// backward while its cursor rests mid-row on a line it is editing.
#[test]
fn a_scroll_down_rotates_the_region_and_leaves_the_cursor_alone() {
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
    let damage = screen.scroll_region_down(1);
    assert_eq!(glyphs(&screen, 4), vec!['a', blank, 'b', 'c']);
    assert_eq!(damage, Some(DamageSpan::Full));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
}

/// Asserts that a count larger than the region blanks the whole
/// region and nothing more.
///
/// Case: an application asks to scroll back by more lines than its
/// region holds.
#[test]
fn a_count_past_the_region_height_scrolls_only_the_region() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    let blank = screen.state.pen.erase_cell().c;
    screen.scroll_region_down(9);
    assert_eq!(glyphs(&screen, 4), vec!['a', blank, blank, 'd']);
}

/// Asserts that a scroll down of the full page feeds nothing to
/// history and leaves a scrolled-back viewport where it was.
///
/// Case: the user is reading scrollback while a program scrolls the
/// full page down by one line.
#[test]
fn a_scroll_down_never_feeds_history() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.set_display_offset(DisplayOffset(1));
    screen.scroll_region_down(1);
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}
