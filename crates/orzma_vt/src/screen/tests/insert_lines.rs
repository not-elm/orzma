//! Tests for inserting blank lines at the cursor inside the scroll
//! region.

use super::*;

/// Asserts that an insert opens a blank row at the cursor, moves the
/// rows below it down inside the region, discards the bottom-margin
/// row, and leaves rows outside the region alone.
///
/// Case: Neovim, with its tabline pinned above the scroll region,
/// scrolls the buffer backward by one line.
#[test]
fn an_insert_opens_a_blank_row_at_the_cursor_inside_the_region() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_lines(1);
    assert_eq!(glyphs(&screen, 4), vec!['a', blank, 'b', 'c']);
    assert_eq!(damage, Some(DamageSpan::Full));
}

/// Asserts that an insert with the cursor outside the scroll region
/// moves nothing and reports nothing.
///
/// Case: an application leaves the cursor on a status row below the
/// region it scrolls and sends a stray insert line.
#[test]
fn an_insert_outside_the_region_does_nothing() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(2),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    let damage = screen.insert_lines(1);
    assert_eq!(glyphs(&screen, 4), vec!['a', 'b', 'c', 'd']);
    assert_eq!(damage, None);
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a count larger than the rows left in the region
/// blanks only those rows and leaves the rows below the bottom margin
/// standing.
///
/// Case: an editor asks to insert more lines than fit between the
/// cursor and the status line it keeps below the region.
#[test]
fn a_count_past_the_bottom_margin_inserts_only_the_remaining_rows() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(2),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(1);
    let blank = screen.state.pen.erase_cell().c;
    screen.insert_lines(9);
    assert_eq!(glyphs(&screen, 4), vec!['a', blank, blank, 'd']);
}

/// Asserts that an insert homes the cursor to column zero and disarms
/// the deferred wrap, leaving the cursor row unchanged.
///
/// Case: a program fills the last column of a row and then inserts a
/// line instead of printing the character the wrap was waiting for.
#[test]
fn an_insert_homes_the_cursor_and_disarms_the_deferred_wrap() {
    let mut screen = screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.insert_lines(1);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(0));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that an insert on the first row of the page feeds nothing
/// to history, because the row it pushes off the bottom margin is
/// discarded rather than scrolled past.
///
/// Case: a program inserts a line at the top of the primary screen
/// while the user has scrollback to return to.
#[test]
fn an_insert_never_feeds_history() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.set_display_offset(DisplayOffset(1));
    screen.state.line = ScreenLine(0);
    screen.insert_lines(1);
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}

/// Asserts that a placement below the cursor follows its row down and
/// one on the bottom-margin row is dropped by the projection and
/// named by the sweep.
///
/// Case: two webviews sit inside an editor's scroll region, one on
/// its last row, when the editor inserts a line above them.
#[test]
fn placements_follow_their_rows_and_the_bottom_margin_row_is_evicted() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(2),
    });
    screen.state.line = ScreenLine(1);
    screen.mount_placement(InstanceId(1), PlacementSize { rows: 1, cols: 1 });
    screen.state.line = ScreenLine(2);
    screen.mount_placement(InstanceId(2), PlacementSize { rows: 1, cols: 1 });
    screen.state.line = ScreenLine(0);
    screen.insert_lines(1);
    let projected = screen.project_placements();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].id, InstanceId(1));
    assert_eq!(projected[0].point.line, GridLine(2));
    assert_eq!(screen.evict_lost_anchors(), vec![InstanceId(2)]);
}
