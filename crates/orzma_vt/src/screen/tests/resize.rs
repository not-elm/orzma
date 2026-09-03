//! Tests for grid resizing.

use super::*;

/// Asserts that a resize to the size the screen already has reports
/// no damage and leaves the cursor alone.
///
/// Case: the window manager replays the same geometry after a focus
/// change, so the host forwards a size the VT already holds.
#[test]
fn a_resize_to_the_current_size_reports_nothing() {
    let mut screen = screen();
    screen.move_cursor_to(Some(2), Some(3));
    assert_eq!(screen.resize(GridSize { cols: 4, rows: 3 }), None);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(2));
}

/// Asserts that a shrink whose cursor already fits drops the bottom
/// rows and leaves the cursor where it was.
///
/// Case: a short `ls` leaves its output near the top of a tall window
/// and the user drags the window shorter.
#[test]
fn a_shrink_that_the_cursor_fits_drops_the_bottom_rows() {
    let mut screen = tall_screen();
    screen.grid[ScreenLine(0)][0].c = 'a';
    screen.state.line = ScreenLine(0);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 2 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
}

/// Asserts that a shrink that would cut the cursor off pushes rows
/// into history and moves the cursor up with its content.
///
/// Case: the shell's prompt sits on the last row of a tall window and
/// the user drags the window shorter.
#[test]
fn a_shrink_that_would_cut_the_cursor_off_scrolls_into_history() {
    let mut screen = tall_screen();
    screen.grid[ScreenLine(3)][0].c = 'd';
    screen.state.line = ScreenLine(3);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 2 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.grid.history_len(), 2);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, 'd');
}

/// Asserts that a growth moves the cursor down by the rows it
/// reclaims from history.
///
/// Case: the user drags a window taller after output has scrolled off
/// the top, and the prompt must stay under the line it follows.
#[test]
fn a_growth_moves_the_cursor_down_with_the_reclaimed_rows() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 4 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(screen.state.line, ScreenLine(3));
}

/// Asserts that a narrowing seats a cursor beyond the new width on
/// the last column rather than leaving it out of bounds.
///
/// Case: the cursor sits at the right edge of a wide window when the
/// user drags it narrower.
#[test]
fn a_narrowing_seats_an_out_of_range_cursor_on_the_last_column() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(19);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 3 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that a width change clears the deferred wrap on both the
/// live cursor and the saved one.
///
/// Case: an application fills the last column, saves the cursor with
/// `DECSC`, and the user widens the window before the next print.
#[test]
fn a_width_change_clears_both_deferred_wraps() {
    let mut screen = screen();
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.save_checkpoint();
    assert_eq!(
        screen.resize(GridSize { cols: 8, rows: 3 }),
        Some(DamageSpan::Full)
    );
    assert!(!screen.state.pending_wrap);
    screen.restore_checkpoint();
    assert!(!screen.state.pending_wrap);
}

/// Asserts that a height-only change leaves the deferred wrap set.
///
/// Case: an application fills the last column and the user drags the
/// window taller without changing its width.
#[test]
fn a_height_only_change_keeps_the_deferred_wrap() {
    let mut screen = screen();
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 4 }),
        Some(DamageSpan::Full)
    );
    assert!(screen.state.pending_wrap);
}

/// Asserts that a height change returns the scroll margins to the
/// whole page so line feeding keeps scrolling afterwards.
///
/// Case: a pager reserves a status line, the user drags the window
/// taller, and the shell that outlives the pager keeps printing.
#[test]
fn a_height_change_returns_the_margins_to_the_whole_page() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 6 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.scroll_region.top_margin(), ScreenLine(0));
    assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(5));
    screen.state.line = ScreenLine(5);
    assert_eq!(screen.line_feed(), Some(DamageSpan::Full));
}

/// Asserts that a height change leaves the cursor origin alone while
/// it resets the margins.
///
/// Case: an application sets origin mode and a scrolling region, and
/// the user resizes the window before it restores either.
#[test]
fn a_height_change_leaves_the_cursor_origin_alone() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    screen.set_origin_mode(OriginMode::WithinMargins);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 6 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::WithinMargins
    );
}

/// Asserts that a saved cursor beyond the new grid is clamped at
/// resize rather than left to seat out of bounds on restore.
///
/// Case: an application saves its cursor near the bottom-right of a
/// large window and the user shrinks the window before `DECRC`.
#[test]
fn a_resize_clamps_the_saved_cursor() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(3);
    screen.save_checkpoint();
    screen.state.line = ScreenLine(0);
    assert_eq!(
        screen.resize(GridSize { cols: 2, rows: 2 }),
        Some(DamageSpan::Full)
    );
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that a growth clamps a scrolled-back viewport to the
/// history that survives it.
///
/// Case: the user is reading scrollback when they drag the window
/// taller, pulling the rows they were looking at back onto the screen.
#[test]
fn a_growth_clamps_the_viewport_to_the_surviving_history() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 2);
    screen.viewport.offset = DisplayOffset(2);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 5 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a shrink that needs no scrolling leaves a
/// scrolled-back viewport where it was.
///
/// Case: the user is reading scrollback with the cursor near the top
/// when they drag the window shorter.
#[test]
fn a_shrink_leaves_the_viewport_offset_alone() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.viewport.offset = DisplayOffset(1);
    screen.state.line = ScreenLine(0);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 2 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.display_offset(), DisplayOffset(1));
}

/// Asserts that a shrink deep enough to scroll keeps a scrolled-back
/// viewport on the row it was showing.
///
/// Case: the user is reading scrollback with the prompt on the last
/// row when they drag the window shorter.
#[test]
fn a_shrink_that_scrolls_carries_a_scrolled_back_viewport_with_it() {
    let mut screen = tall_screen();
    screen.grid[ScreenLine(0)][0].c = 'a';
    screen.state.line = ScreenLine(3);
    screen.line_feed();
    screen.viewport.offset = DisplayOffset(1);
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 2 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.display_offset(), DisplayOffset(3));
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
}

/// Asserts that the rows a resize creates carry default cells rather
/// than the pen's background.
///
/// Case: an application selects a red background, prints, and the user
/// drags the window taller before the application restores the pen.
#[test]
fn a_resize_fills_new_rows_with_default_cells() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(1);
    assert_eq!(
        screen.resize(GridSize { cols: 6, rows: 4 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.viewport_row(ViewportLine(3))[0], Cell::default());
    assert_eq!(screen.viewport_row(ViewportLine(0))[5], Cell::default());
}

/// Asserts that a narrowing does not push the columns it drops onto
/// the row below.
///
/// Case: a long line fills a wide window and the user drags the
/// window narrow enough to cut it.
#[test]
fn a_narrowing_does_not_wrap_the_dropped_columns_onto_the_next_row() {
    let mut screen = wide_screen();
    screen.grid[ScreenLine(0)][4].c = 'x';
    screen.grid[ScreenLine(0)][5].c = 'y';
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 3 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.viewport_row(ViewportLine(1))[0], Cell::default());
    assert_eq!(screen.viewport_row(ViewportLine(1))[1], Cell::default());
}

/// Asserts that a placement anchored beyond the new right edge stays
/// mounted rather than being culled by the resize.
///
/// Case: a webview is mounted near the right edge of a wide window and
/// the user drags the window narrower.
#[test]
fn a_placement_beyond_the_new_right_edge_survives() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(19);
    screen.mount_placement(InstanceId(1), PlacementSize { rows: 1, cols: 1 });
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 3 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.placement_count(), 1);
    assert!(screen.evict_lost_anchors().is_empty());
    assert_eq!(screen.project_placements()[0].point.column, GridColumn(19));
}

/// Asserts that a growth moves the saved cursor down with the rows it
/// reclaims, so a restore lands on the row that was saved rather than
/// the reclaimed rows above it.
///
/// Case: a shell saves its cursor on the prompt row before a
/// full-screen program takes over, and the user drags the window
/// taller over output that had scrolled off the top before the program
/// exits and restores it.
#[test]
fn a_growth_moves_the_saved_cursor_down_with_the_reclaimed_rows() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.line_feed();
    screen.state.column = GridColumn(1);
    screen.save_checkpoint();
    assert_eq!(screen.grid.history_len(), 2);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 5 }),
        Some(DamageSpan::Full)
    );
    screen.state.line = ScreenLine(0);
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(4));
    assert_eq!(screen.state.column, GridColumn(1));
}

/// Asserts that a shrink that scrolls rows into history moves the saved
/// cursor up with the row it sits on.
///
/// Case: a program saves its cursor above the bottom row, and the user
/// drags the window short enough that rows scroll off the top before
/// the program restores it.
#[test]
fn a_shrink_that_scrolls_moves_the_saved_cursor_up_with_its_row() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(2);
    screen.save_checkpoint();
    screen.state.line = ScreenLine(3);
    assert_eq!(
        screen.resize(GridSize { cols: 4, rows: 2 }),
        Some(DamageSpan::Full)
    );
    assert_eq!(screen.state.line, ScreenLine(1));
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(0));
}
