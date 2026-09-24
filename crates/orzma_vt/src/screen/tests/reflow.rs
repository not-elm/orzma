//! Tests for resizing with the rows rewrapped at the new width.

use super::*;
use crate::screen::grid::reflow::ScrollbackOnGrow;

fn sized(cols: u16, rows: u16, max_history: usize) -> Screen {
    Screen::new(GridSize { cols, rows }, max_history)
}

fn reflow(screen: &mut Screen, cols: u16, rows: u16) -> Option<DamageSpan> {
    screen.reflow(GridSize { cols, rows }, ScrollbackOnGrow::Keep)
}

/// Asserts that narrowing wraps the prompt onto the next row and
/// widening back joins it again, with the cursor following its text.
///
/// Case: the user narrows a Windows terminal until the prompt no longer
/// fits, then widens it back.
#[test]
fn narrowing_then_widening_restores_the_prompt() {
    let mut screen = sized(8, 3, 10);
    print_text(&mut screen, "PS C:\\>");
    assert_eq!(reflow(&mut screen, 4, 3), Some(DamageSpan::Full));
    assert_eq!(row_text(&screen, 0), "PS C");
    assert_eq!(row_text(&screen, 1), ":\\>");
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(1), GridColumn(3))
    );
    reflow(&mut screen, 8, 3);
    assert_eq!(row_text(&screen, 0), "PS C:\\>");
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(0), GridColumn(7))
    );
}

/// Asserts that a resize to the current size reports nothing.
///
/// Case: the window manager replays the same geometry after a focus
/// change.
#[test]
fn a_reflow_to_the_current_size_reports_nothing() {
    let mut screen = sized(8, 3, 10);
    assert_eq!(reflow(&mut screen, 8, 3), None);
}

/// Asserts that a parked cursor stays parked on the right edge, and the
/// next glyph wraps and records the wrap.
///
/// Case: a prompt fills its row exactly, the user narrows the window,
/// then types.
#[test]
fn a_parked_cursor_stays_parked_and_the_next_glyph_wraps() {
    let mut screen = sized(8, 3, 10);
    print_text(&mut screen, "abcdefgh");
    reflow(&mut screen, 4, 3);
    assert_eq!(screen.cursors()[0], (ScreenLine(1), GridColumn(3), true));
    print_text(&mut screen, "x");
    assert_eq!(row_text(&screen, 2), "x");
    assert_eq!(screen.grid.wrap_at(GridLine(1)), Some(4));
}

/// Asserts that a height-only resize leaves a parked cursor parked.
///
/// Case: a prompt fills its row exactly and the user drags the window
/// taller.
#[test]
fn a_height_only_reflow_keeps_the_deferred_wrap() {
    let mut screen = sized(4, 3, 10);
    print_text(&mut screen, "abcd");
    screen.reflow(GridSize { cols: 4, rows: 5 }, ScrollbackOnGrow::Reclaim);
    assert!(screen.state.pending_wrap);
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that the saved cursor lands on the top row when its row is
/// pushed into history, so a restore stays inside the screen.
///
/// Case: a program saves the cursor at the top of the screen, and the
/// user narrows the window until that row scrolls off.
#[test]
fn a_saved_cursor_pushed_into_history_lands_on_the_top_row() {
    let mut screen = sized(4, 2, 10);
    screen.save_checkpoint();
    print_text(&mut screen, "abcdefgh");
    reflow(&mut screen, 2, 2);
    screen.restore_checkpoint();
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(0), GridColumn(0))
    );
}

/// Asserts that a selection follows the text it covers across a
/// widening.
///
/// Case: the user selects the wrapped half of a long command and widens
/// the window before copying.
#[test]
fn a_selection_follows_its_text() {
    let mut screen = sized(4, 3, 10);
    print_text(&mut screen, "abcdef");
    screen.start_selection(point(1, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 1), CellSide::Right);
    reflow(&mut screen, 8, 3);
    assert_eq!(screen.selection_text().as_deref(), Some("ef"));
    let range = screen.selection_range().expect("the selection survives");
    assert_eq!((range.start, range.end), (point(0, 4), point(0, 5)));
}

/// Asserts that a selection reaching past the end of a short line does
/// not change how the rows rewrap.
///
/// Case: the user drags a selection to the right edge of a prompt and
/// narrows the window.
#[test]
fn a_selection_does_not_change_the_layout() {
    let mut plain = sized(8, 3, 10);
    let mut selected = sized(8, 3, 10);
    print_text(&mut plain, "ab\ncd");
    print_text(&mut selected, "ab\ncd");
    selected.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    selected.extend_selection(point(0, 7), CellSide::Right);
    reflow(&mut plain, 4, 3);
    reflow(&mut selected, 4, 3);
    let rows = |screen: &Screen| {
        (0..3)
            .map(|line| row_text(screen, line))
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&plain), rows(&selected));
    assert_eq!(selected.selection_text().as_deref(), Some("ab"));
}

/// Asserts that a selection end on a row dropped off the bottom stops
/// resolving without a panic.
///
/// Case: the user selects text below the prompt and shrinks the window
/// until that row falls off.
#[test]
fn a_selection_end_dropped_off_the_bottom_stops_resolving() {
    let mut screen = sized(4, 3, 10);
    print_text(&mut screen, "ab\ncd\nef");
    screen.move_cursor_to(Some(1), Some(1));
    screen.start_selection(point(2, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(2, 1), CellSide::Right);
    reflow(&mut screen, 4, 1);
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that a placement follows the text its anchor sits on.
///
/// Case: a webview is mounted on the wrapped half of a command and the
/// user widens the window.
#[test]
fn a_placement_follows_its_text() {
    let mut screen = sized(4, 3, 10);
    print_text(&mut screen, "abcdef");
    let id = InstanceId(1);
    screen.mount_placement_at(
        id,
        ScreenLine(1),
        GridColumn(1),
        PlacementSize { rows: 1, cols: 1 },
    );
    reflow(&mut screen, 8, 3);
    let placements = screen.project_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].point, point(0, 5));
}

/// Asserts that a placement whose row the history cap drops is evicted.
///
/// Case: a webview is mounted near the top of a terminal with scrollback
/// off, and the user narrows the window until its row scrolls away.
#[test]
fn a_placement_on_a_row_past_the_cap_is_evicted() {
    let mut screen = sized(4, 2, 0);
    let id = InstanceId(1);
    screen.mount_placement(id, PlacementSize { rows: 1, cols: 1 });
    print_text(&mut screen, "abcdefgh");
    reflow(&mut screen, 2, 2);
    assert_eq!(screen.evict_lost_anchors(), vec![id]);
}

/// Asserts that a placement on a row a narrowing cuts off the bottom is
/// evicted, even though the first row of its line survives.
///
/// Case: a webview is mounted near the end of a long line on a short
/// window, and the user narrows the window until that part falls off.
#[test]
fn a_placement_on_a_dropped_cut_row_is_evicted() {
    let mut screen = sized(8, 2, 10);
    print_text(&mut screen, "abcdefgh");
    screen.move_cursor_to(Some(1), Some(1));
    let id = InstanceId(1);
    screen.mount_placement_at(
        id,
        ScreenLine(0),
        GridColumn(6),
        PlacementSize { rows: 1, cols: 1 },
    );
    reflow(&mut screen, 2, 2);
    assert_eq!(screen.evict_lost_anchors(), vec![id]);
}

/// Asserts that a selection with an end on a row a narrowing cuts off
/// the bottom is cleared rather than resolving on the line's first row.
///
/// Case: the user selects the end of a long line on a short window and
/// narrows the window until that part falls off.
#[test]
fn a_selection_on_a_dropped_cut_row_is_cleared() {
    let mut screen = sized(8, 2, 10);
    print_text(&mut screen, "abcdefgh");
    screen.move_cursor_to(Some(1), Some(1));
    screen.start_selection(point(0, 6), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 7), CellSide::Right);
    reflow(&mut screen, 2, 2);
    assert_eq!(screen.selection_range(), None);
    assert_eq!(screen.selection_text(), None);
}

/// Asserts that a scrolled-back viewport keeps the row it showed at its
/// top.
///
/// Case: the user scrolls back to read earlier output and narrows the
/// window.
#[test]
fn a_scrolled_back_viewport_keeps_its_top_row() {
    let mut screen = sized(4, 2, 10);
    print_text(&mut screen, "ab\ncd\nef\ngh");
    screen.set_display_offset(DisplayOffset(1));
    reflow(&mut screen, 2, 2);
    assert_eq!(screen.display_offset(), DisplayOffset(2));
    assert_eq!(screen.viewport_row(ViewportLine(0)).text(), "cd");
}

/// Asserts that a viewport whose top row the history cap drops moves to
/// the oldest row instead of failing.
///
/// Case: the user reads the oldest scrollback of a short-scrollback
/// terminal and narrows the window.
#[test]
fn a_viewport_whose_top_row_is_dropped_moves_to_the_oldest_row() {
    let mut screen = sized(4, 2, 2);
    print_text(&mut screen, "ab\ncd\nef\ngh");
    screen.set_display_offset(DisplayOffset(2));
    reflow(&mut screen, 2, 2);
    assert_eq!(screen.display_offset(), DisplayOffset(2));
    assert_eq!(screen.viewport_row(ViewportLine(0)).text(), "cd");
}

/// Asserts that a saved cursor whose column falls past the new width is
/// clamped onto the last column without arming the deferred wrap.
///
/// Case: a program saves the cursor near the right edge of a blank row
/// below the prompt, the user narrows the window, and the program
/// restores it.
#[test]
fn a_saved_cursor_past_the_new_width_does_not_arm_the_deferred_wrap() {
    let mut screen = sized(8, 3, 10);
    print_text(&mut screen, "ab");
    screen.move_cursor_to(Some(2), Some(7));
    screen.save_checkpoint();
    screen.move_cursor_to(Some(1), Some(3));
    reflow(&mut screen, 4, 3);
    screen.restore_checkpoint();
    assert_eq!(screen.cursors()[0], (ScreenLine(1), GridColumn(3), false));
}

/// Asserts that a saved cursor seated on the top row because its own row
/// moved into history does not arm the deferred wrap there.
///
/// Case: a program saves the cursor at the right edge of a full row and
/// prints two more lines, the user narrows the window, and the program
/// restores the cursor and prints.
#[test]
fn a_saved_cursor_moved_off_its_row_does_not_arm_the_deferred_wrap() {
    let mut screen = sized(4, 3, 10);
    print_text(&mut screen, "abcd");
    screen.save_checkpoint();
    print_text(&mut screen, "\n12\n34");
    reflow(&mut screen, 2, 3);
    screen.restore_checkpoint();
    print_text(&mut screen, "x");
    assert_eq!(row_text(&screen, 0), "1x");
    assert_eq!(screen.grid.wrap_at(GridLine(0)), None);
}

/// Asserts that a whole-line selection keeps every row its line rewraps
/// into rather than only the row holding the click.
///
/// Case: the user triple-clicks a long line of output and narrows the
/// window before copying it.
#[test]
fn a_whole_line_selection_keeps_its_rewrapped_rows() {
    let mut screen = sized(8, 3, 10);
    print_text(&mut screen, "abcdefgh\nxy");
    screen.start_selection(point(0, 5), CellSide::Left, SelectionKind::Lines);
    reflow(&mut screen, 4, 3);
    assert_eq!(screen.selection_text().as_deref(), Some("abcdefgh"));
}

/// Asserts that a selection starting on the right edge of a row that ends
/// its line still starts on the next line after a widening.
///
/// Case: the user starts a drag in the padding right of a full line and
/// widens the window before copying.
#[test]
fn a_selection_starting_past_a_line_end_keeps_its_text() {
    let mut screen = sized(4, 4, 10);
    print_text(&mut screen, "abcd\nefgh");
    screen.start_selection(point(0, 3), CellSide::Right, SelectionKind::Simple);
    screen.extend_selection(point(1, 1), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("ef"));
    reflow(&mut screen, 8, 4);
    assert_eq!(screen.selection_text().as_deref(), Some("ef"));
}

/// Asserts that the blanks a narrowing keeps before the cursor and splits
/// across a soft wrap are left out of the copy.
///
/// Case: the cursor sits past the end of a short line when the user selects
/// it together with the line below and narrows the window before copying.
#[test]
fn blanks_kept_for_the_cursor_are_left_out_of_the_copy() {
    let mut screen = sized(8, 4, 10);
    print_text(&mut screen, "ab\ncd");
    screen.move_cursor_to(Some(1), Some(6));
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 1), CellSide::Right);
    reflow(&mut screen, 4, 4);
    assert_eq!(screen.selection_text().as_deref(), Some("ab\ncd"));
}

/// Asserts that a cursor a backward tab left off the last column with the
/// deferred wrap armed keeps its column, and its armed wrap, across a
/// height-only reflow.
///
/// Case: a program fills a row, tabs backward, and the user drags the
/// window taller before the program prints again.
#[test]
fn a_cursor_armed_off_the_last_column_keeps_its_column() {
    let mut screen = sized(20, 3, 10);
    print_text(&mut screen, "abcdefghijklmnopqrst");
    screen.move_backward_tabs(1);
    assert_eq!(
        (screen.state.column, screen.state.pending_wrap),
        (GridColumn(16), true)
    );
    screen.reflow(GridSize { cols: 20, rows: 4 }, ScrollbackOnGrow::Reclaim);
    assert_eq!(screen.cursors()[0], (ScreenLine(0), GridColumn(16), true));
}

/// Asserts that a reflow keeps the background of the blank rows below the
/// cursor rather than resetting it.
///
/// Case: a script paints the whole screen blue with a clear, and the user
/// drags the window taller.
#[test]
fn a_reflow_keeps_the_colors_of_blank_rows_below_the_cursor() {
    let mut screen = sized(6, 4, 10);
    screen.pen_mut().bg = Color::Indexed(4);
    let _ = screen.erase_in_display(EraseScreenMode::All);
    screen.reflow(GridSize { cols: 6, rows: 5 }, ScrollbackOnGrow::Reclaim);
    for line in 0..4 {
        assert!(
            screen
                .grid
                .row(GridLine(line))
                .iter()
                .all(|cell| cell.bg == Color::Indexed(4)),
            "row {line} lost its background"
        );
    }
}
