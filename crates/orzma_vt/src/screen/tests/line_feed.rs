//! Tests for line feeding and the region scrolling it triggers.

use super::*;

/// Asserts that a linefeed above the bottom row only moves the
/// cursor and reports no damage.
///
/// Case: a shell prints multiple output lines while the screen
/// still has empty rows below the cursor.
#[test]
fn a_linefeed_above_the_bottom_moves_the_cursor() {
    let mut screen = screen();
    let damage = screen.line_feed();
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(damage, None);
}

/// Asserts that a linefeed at the bottom margin scrolls the screen and
/// pushes the departing row into history.
///
/// Case: a shell prints past the last row and the earlier output has to
/// remain reachable by scrolling back.
#[test]
fn a_bottom_linefeed_scrolls_and_pushes_history() {
    let mut screen = screen();
    screen.grid[ScreenLine(0)][GridColumn(0)].c = 'a';
    screen.state.line = ScreenLine(2);
    assert_eq!(screen.line_feed(), Some(DamageSpan::Full));
    assert_eq!(screen.grid.history_len(), 1);
}

/// Asserts that the row scrolled in at the bottom carries the
/// current pen background.
///
/// Case: an application sets a colored background and scrolls at
/// the bottom of the screen.
#[test]
fn a_scrolled_in_row_carries_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid[ScreenLine(2)][0].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(2)][3].bg, Color::Indexed(4));
}

/// Asserts that a linefeed below the bottom margin moves the
/// cursor down and scrolls nothing.
///
/// The agreed policy gates the scroll on the cursor sitting
/// exactly at the bottom margin, the way VT510 writes IND and
/// NEL, rather than on the cursor having reached it: a cursor
/// outside the region moves like an ordinary cursor-down instead
/// of scrolling rows it is not among.
///
/// Case: an application reserves a two-row footer below its
/// scrolling pane and emits a linefeed while the cursor rests on
/// the footer's first row.
#[test]
fn a_linefeed_below_a_bottom_margin_moves_the_cursor_down() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(1),
    });
    screen.state.line = ScreenLine(2);
    let damage = screen.line_feed();
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(damage, None);
}

/// Asserts that a linefeed below the bottom margin, already on
/// the last row, moves and scrolls nothing.
///
/// The agreed policy mirrors the reverse index above a top
/// margin: a cursor that hits the screen edge outside the
/// scrolling region stays put, rather than scrolling the region
/// it is not inside.
///
/// Case: an application reserves a footer below its scrolling
/// pane and emits a linefeed while the cursor rests on the last
/// row of that footer.
#[test]
fn a_linefeed_below_a_bottom_margin_at_the_last_row_does_nothing() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(1),
    });
    screen.grid[ScreenLine(0)][0].c = 'a';
    screen.state.line = ScreenLine(3);
    let damage = screen.line_feed();
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(damage, None);
}

/// Asserts that a linefeed at the bottom of a region below a
/// non-zero top margin rotates the region and leaves history and
/// the rows above it alone.
///
/// The agreed policy feeds scrollback only when the top margin is
/// row zero, following alacritty: rows leaving a region that has
/// content pinned above it never reached the top of the screen,
/// so treating them as scrollback would interleave them with
/// output the user never scrolled past.
///
/// Case: an application pins a header on the first row and
/// scrolls the pane below it forward.
#[test]
fn a_linefeed_below_a_top_margin_rotates_without_feeding_history() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c'), (3, 'd')] {
        screen.grid[ScreenLine(line)][0].c = glyph;
    }
    screen.state.line = ScreenLine(3);
    let blank = screen.state.pen.erase_cell().c;
    screen.line_feed();
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'c');
    assert_eq!(screen.grid[ScreenLine(2)][0].c, 'd');
    assert_eq!(screen.grid[ScreenLine(3)][0].c, blank);
    assert_eq!(screen.grid.history_len(), 0);
}

/// Asserts that a linefeed at a bottom margin above the last row
/// still feeds history and leaves the rows below the margin
/// standing.
///
/// Case: an application keeps a status line on the last row and
/// scrolls the pane above it forward.
#[test]
fn a_linefeed_at_a_bottom_margin_feeds_history_and_holds_the_rows_below() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(2),
    });
    for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c'), (3, 'd')] {
        screen.grid[ScreenLine(line)][0].c = glyph;
    }
    screen.state.line = ScreenLine(2);
    let blank = screen.state.pen.erase_cell().c;
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'b');
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'c');
    assert_eq!(screen.grid[ScreenLine(2)][0].c, blank);
    assert_eq!(screen.grid[ScreenLine(3)][0].c, 'd');
    assert_eq!(screen.grid.row(GridLine(-1))[0].c, 'a');
}

/// Asserts that a linefeed preserves the deferred-wrap flag.
///
/// The agreed policy follows alacritty: only a carriage return or
/// an explicit cursor motion clears the pending wrap; a bare
/// linefeed does not.
///
/// Case: an application writes a full-width line, then emits a bare
/// linefeed before continuing to print on the next row.
#[test]
fn a_linefeed_preserves_pending_wrap() {
    let mut screen = screen();
    screen.state.pending_wrap = true;
    screen.line_feed();
    assert!(screen.state.pending_wrap);
}
