//! Tests for deleting lines at the cursor inside the scroll region.

use super::*;

/// Asserts that a delete moves the rows below the cursor up inside
/// the region, blanks the bottom-margin row with the pen's erase cell,
/// leaves rows outside the region alone, and feeds no history.
///
/// Case: Neovim, with its tabline pinned above the scroll region,
/// scrolls the buffer forward by one line.
#[test]
fn a_delete_moves_the_rows_below_the_cursor_up_inside_the_region() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_lines(1);
    assert_eq!(glyphs(&screen, 4), vec!['a', 'c', 'd', blank]);
    assert_eq!(damage, Some(DamageSpan::Full));
    assert_eq!(screen.grid.history_len(), 0);
}

/// Asserts that a delete with the cursor outside the scroll region
/// moves nothing and reports nothing.
///
/// Case: an application leaves the cursor on a header row above the
/// region it scrolls and sends a stray delete line.
#[test]
fn a_delete_outside_the_region_does_nothing() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(2);
    let damage = screen.delete_lines(1);
    assert_eq!(glyphs(&screen, 4), vec!['a', 'b', 'c', 'd']);
    assert_eq!(damage, None);
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a count larger than the rows left in the region
/// deletes only those rows and leaves the rows below the bottom
/// margin standing.
///
/// Case: an editor asks to delete more lines than fit between the
/// cursor and the status line it keeps below the region.
#[test]
fn a_count_past_the_bottom_margin_deletes_only_the_remaining_rows() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(0),
        bottom: ScreenLine(2),
    });
    seed(&mut screen, &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(1);
    let blank = screen.state.pen.erase_cell().c;
    screen.delete_lines(9);
    assert_eq!(glyphs(&screen, 4), vec!['a', blank, blank, 'd']);
}

/// Asserts that the rows a delete opens carry the pen's background
/// rather than a default cell, so background-colour erase holds.
///
/// Case: a program paints a pane with a blue background and then
/// deletes a line inside it.
#[test]
fn a_delete_fills_the_opened_rows_with_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.line = ScreenLine(0);
    screen.delete_lines(1);
    let opened = &screen.grid[ScreenLine(2)];
    assert_eq!(opened[0].c, ' ');
    assert_eq!(opened[0].bg, Color::Indexed(4));
}

/// Asserts that a delete homes the cursor to column zero and disarms
/// the deferred wrap, leaving the cursor row unchanged.
///
/// Case: a program fills the last column of a row and then deletes
/// the line below instead of printing the character the wrap was
/// waiting for.
#[test]
fn a_delete_homes_the_cursor_and_disarms_the_deferred_wrap() {
    let mut screen = screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.delete_lines(1);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(0));
    assert!(!screen.state.pending_wrap);
}

/// Asserts that a zero count deletes nothing and leaves the cursor
/// where it was.
///
/// Case: a caller forwards a count of zero straight through instead
/// of applying the control function's default of one.
#[test]
fn a_zero_count_deletes_nothing() {
    let mut screen = screen();
    seed(&mut screen, &['a', 'b', 'c']);
    screen.state.column = GridColumn(2);
    let damage = screen.delete_lines(0);
    assert_eq!(glyphs(&screen, 3), vec!['a', 'b', 'c']);
    assert_eq!(damage, None);
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a delete with the cursor on the first row of a
/// full-page region feeds every deleted row to history and walks a
/// scrolled-back viewport one row further per deleted row.
///
/// Case: the user is reading scrollback on the primary screen while
/// a program deletes two lines from the top of the page.
#[test]
fn a_delete_on_the_first_row_feeds_history_and_holds_a_scrolled_viewport_per_row() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 2);
    screen.set_display_offset(DisplayOffset(2));
    seed(&mut screen, &['a', 'b', 'c']);
    screen.state.line = ScreenLine(0);
    let blank = screen.state.pen.erase_cell().c;
    screen.delete_lines(2);
    assert_eq!(screen.grid.history_len(), 4);
    assert_eq!(screen.grid.row(GridLine(-2))[0].c, 'a');
    assert_eq!(screen.grid.row(GridLine(-1))[0].c, 'b');
    assert_eq!(glyphs(&screen, 3), vec!['c', blank, blank]);
    assert_eq!(screen.display_offset(), DisplayOffset(4));
}

/// Asserts that at history capacity a delete on the first row keeps
/// the viewport offset clamped to the history that survives.
///
/// Case: the user is reading the oldest line of a small scrollback
/// when a program deletes two lines from the top of the page.
#[test]
fn a_delete_at_history_capacity_clamps_the_viewport_to_surviving_history() {
    let mut screen = Screen::new(GridSize { cols: 4, rows: 3 }, 1);
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.set_display_offset(DisplayOffset(1));
    screen.state.line = ScreenLine(0);
    screen.delete_lines(2);
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}

/// Asserts that a viewport pinned to the live tail stays pinned when
/// a delete on the first row feeds history.
///
/// Case: the user is watching live output when a program deletes a
/// line from the top of the page.
#[test]
fn a_delete_on_the_first_row_leaves_a_pinned_viewport_pinned() {
    let mut screen = screen();
    screen.state.line = ScreenLine(0);
    screen.delete_lines(1);
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a placement anchored below the deleted rows follows
/// its row up the screen.
///
/// Case: a webview sits under a line of output and the program
/// deletes a line above it.
#[test]
fn a_placement_below_the_deleted_rows_follows_its_row() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.mount_placement(InstanceId(1), PlacementSize { rows: 1, cols: 1 });
    screen.state.line = ScreenLine(1);
    screen.delete_lines(1);
    let projected = screen.project_placements();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].point.line, GridLine(1));
    assert!(screen.evict_lost_anchors().is_empty());
}

/// Asserts that a placement anchored on a deleted row inside a region
/// below a top margin is dropped by the projection and named by the
/// sweep.
///
/// Case: a webview sits on a line inside an editor's scroll region and
/// the editor deletes that line.
#[test]
fn a_placement_on_a_deleted_row_is_evicted() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    screen.state.line = ScreenLine(2);
    screen.mount_placement(InstanceId(1), PlacementSize { rows: 1, cols: 1 });
    screen.delete_lines(1);
    assert!(screen.project_placements().is_empty());
    assert_eq!(screen.evict_lost_anchors(), vec![InstanceId(1)]);
}

/// Asserts that a selection below the deleted rows follows its rows
/// and one confined to a recycled row stops resolving.
///
/// Case: the user has text selected in an editor pane when the editor
/// deletes lines above and on the selection.
#[test]
fn a_selection_follows_surviving_rows_and_unresolves_on_a_recycled_one() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(3),
    });
    let point = |line: i32, column: u16| GridPoint {
        line: GridLine(line),
        column: GridColumn(column),
    };
    assert!(screen.start_selection(point(3, 0), CellSide::Left, SelectionKind::Simple));
    assert!(screen.extend_selection(point(3, 2), CellSide::Right));
    screen.state.line = ScreenLine(1);
    screen.delete_lines(1);
    let range = screen
        .selection_range()
        .expect("the selection followed its row");
    assert_eq!(range.start.line, GridLine(2));
    assert_eq!(range.end.line, GridLine(2));

    assert!(screen.start_selection(point(1, 0), CellSide::Left, SelectionKind::Simple));
    assert!(screen.extend_selection(point(1, 2), CellSide::Right));
    screen.delete_lines(1);
    assert_eq!(screen.selection_range(), None);
}
