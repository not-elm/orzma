//! Unit tests for the vi mode of [`Screen`].

use super::*;
use crate::screen::grid::GridSize;
use crate::screen::grid::reflow::ScrollbackOnGrow;
use crate::screen::selection::{CellSide, SelectionKind};
use crate::screen::tests::{point, print_text};
use crate::screen::viewport::DisplayOffset;

fn screen(cols: u16, rows: u16, max_history: usize) -> Screen {
    Screen::new(GridSize { cols, rows }, max_history)
}

fn vi_point(screen: &Screen) -> Option<GridPoint> {
    screen.vi_cursor().map(|cursor| cursor.point)
}

fn chars() -> SemanticEscapeChars {
    SemanticEscapeChars::default()
}

/// Asserts that entering vi mode seats the vi cursor on the write
/// cursor.
///
/// Case: the shell has printed a prompt and the user enters vi mode.
#[test]
fn entering_seats_the_vi_cursor_on_the_write_cursor() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "ab");
    assert!(screen.enter_vi_mode());
    assert_eq!(vi_point(&screen), Some(point(0, 2)));
}

/// Asserts that entering vi mode while the viewport is scrolled back
/// past the write cursor seats the vi cursor on the viewport's top-left
/// cell.
///
/// Case: the user has scrolled back through output with the wheel and
/// then enters vi mode.
#[test]
fn entering_while_scrolled_past_the_write_cursor_seats_it_top_left() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4\n5");
    let _ = screen.scroll(Scroll::Delta(2));
    assert!(screen.enter_vi_mode());
    assert_eq!(vi_point(&screen), Some(point(-2, 0)));
}

/// Asserts that entering vi mode twice reports no change the second
/// time and leaves the vi cursor where it was.
///
/// Case: the vi-mode shortcut is pressed while vi mode is already on.
#[test]
fn entering_twice_reports_no_change() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "ab");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 0));
    assert!(!screen.enter_vi_mode());
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that entering vi mode drops an existing selection.
///
/// Case: a mouse selection is still highlighted when the user enters vi
/// mode.
#[test]
fn entering_drops_the_selection() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "hello");
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Right);
    assert!(screen.enter_vi_mode());
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that leaving vi mode drops the vi cursor and the selection and
/// returns a scrolled-back viewport to the live tail, owing a full
/// repaint.
///
/// Case: the user leaves vi mode after browsing scrollback with a
/// selection highlighted.
#[test]
fn exiting_returns_to_the_live_tail_and_drops_the_selection() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4\n5");
    assert!(screen.enter_vi_mode());
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Lines);
    let _ = screen.scroll(Scroll::Delta(2));
    assert_eq!(screen.exit_vi_mode(), ViewChange::Repainted);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
    assert_eq!(screen.vi_cursor(), None);
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that leaving vi mode outside vi mode changes nothing.
///
/// Case: a stale exit request arrives for a terminal that already left
/// vi mode.
#[test]
fn exiting_outside_vi_mode_changes_nothing() {
    let mut screen = screen(10, 3, 10);
    assert_eq!(screen.exit_vi_mode(), ViewChange::Unchanged);
}

/// Asserts that leaving vi mode at the live tail changes what a frame
/// carries without repainting any row.
///
/// Case: the user enters vi mode on the prompt and leaves it without
/// scrolling back.
#[test]
fn exiting_at_the_live_tail_repaints_nothing() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "ab");
    assert!(screen.enter_vi_mode());
    assert_eq!(screen.exit_vi_mode(), ViewChange::Carried);
}

/// Asserts that a change is classified by whether the viewport moved
/// first and by whether carried state changed second.
///
/// Case: the user presses `k` on the viewport's top row with scrollback
/// above it, so one motion both moves the vi cursor and scrolls the
/// viewport.
#[test]
fn a_view_change_is_classified_by_the_viewport_first() {
    assert_eq!(ViewChange::classify(false, None), ViewChange::Unchanged);
    assert_eq!(ViewChange::classify(true, None), ViewChange::Carried);
    assert_eq!(
        ViewChange::classify(false, Some(DamageSpan::Full)),
        ViewChange::Repainted
    );
    assert_eq!(
        ViewChange::classify(true, Some(DamageSpan::Full)),
        ViewChange::Repainted
    );
    assert_eq!(ViewChange::Repainted.damage(), Some(DamageSpan::Full));
    assert_eq!(ViewChange::Carried.damage(), None);
    assert!(ViewChange::Carried.is_changed());
    assert!(!ViewChange::Unchanged.is_changed());
}

/// Asserts that a vi cursor stored on the continuation column of a wide
/// glyph is reported on the glyph's body.
///
/// Case: output overwrites the cell under the vi cursor with a wide
/// glyph whose second column lands under it.
#[test]
fn a_vi_cursor_on_a_continuation_column_reports_the_glyph_body() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "あ");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 1));
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that a motion that carries the vi cursor above the viewport
/// scrolls the viewport just far enough to show it, owing a full repaint.
///
/// Case: the user holds `k` from the top row of the screen into
/// scrollback.
#[test]
fn a_motion_above_the_viewport_scrolls_it_to_show_the_vi_cursor() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4");
    assert!(screen.enter_vi_mode());
    let _ = screen.vi_motion(ViMotion::Up, &chars());
    let _ = screen.vi_motion(ViMotion::Up, &chars());
    assert_eq!(
        screen.vi_motion(ViMotion::Up, &chars()),
        ViewChange::Repainted
    );
    assert_eq!(screen.display_offset(), DisplayOffset(1));
    assert_eq!(vi_point(&screen), Some(point(-1, 1)));
}

/// Asserts that a motion that carries the vi cursor below a scrolled-back
/// viewport scrolls it toward the live tail just far enough.
///
/// Case: the user browsing scrollback presses `j` on the bottom visible
/// row.
#[test]
fn a_motion_below_the_viewport_scrolls_it_toward_the_live_tail() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4");
    assert!(screen.enter_vi_mode());
    let _ = screen.scroll(Scroll::Delta(1));
    screen.vi.set(point(1, 0));
    assert_eq!(
        screen.vi_motion(ViMotion::Down, &chars()),
        ViewChange::Repainted
    );
    assert_eq!(screen.display_offset(), DisplayOffset(0));
    assert_eq!(vi_point(&screen), Some(point(2, 0)));
}

/// Asserts that a motion inside the viewport owes no damage.
///
/// Case: the user presses `h` on the prompt line.
#[test]
fn a_motion_inside_the_viewport_owes_no_damage() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "abc");
    assert!(screen.enter_vi_mode());
    assert_eq!(
        screen.vi_motion(ViMotion::Left, &chars()),
        ViewChange::Carried
    );
    assert_eq!(vi_point(&screen), Some(point(0, 2)));
}

/// Asserts that a motion outside vi mode changes nothing.
///
/// Case: a motion request arrives just after the terminal left vi mode.
#[test]
fn a_motion_outside_vi_mode_changes_nothing() {
    let mut screen = screen(10, 3, 10);
    assert_eq!(
        screen.vi_motion(ViMotion::Left, &chars()),
        ViewChange::Unchanged
    );
}

/// A 10×3 screen whose rows read `4` / `5` / `6` over three rows of
/// history, `1` / `2` / `3`, in vi mode with the vi cursor on the write
/// cursor at (2, 1).
fn scrolled_screen() -> Screen {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4\n5\n6");
    assert!(screen.enter_vi_mode());
    screen
}

/// Asserts that a line scroll keeps the vi cursor's row and pushes it back
/// inside the viewport.
///
/// Case: the user turns the wheel up two notches in vi mode.
#[test]
fn a_line_scroll_pushes_the_vi_cursor_into_the_viewport() {
    let mut screen = scrolled_screen();
    assert_eq!(
        screen.vi_scroll(Scroll::Delta(2), &chars()),
        ViewChange::Repainted
    );
    assert_eq!(vi_point(&screen), Some(point(0, 1)));
}

/// Asserts that a line scroll that leaves the vi cursor inside the
/// viewport still repaints every row.
///
/// Case: the user turns the wheel up one notch while the vi cursor sits on
/// the top row of the screen.
#[test]
fn a_line_scroll_that_keeps_the_vi_cursor_in_view_repaints() {
    let mut screen = scrolled_screen();
    screen.vi.set(point(0, 0));
    assert_eq!(
        screen.vi_scroll(Scroll::Delta(1), &chars()),
        ViewChange::Repainted
    );
    assert_eq!(screen.display_offset(), DisplayOffset(1));
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that a page scroll moves the vi cursor by a screenful onto the
/// first non-blank cell of its new row.
///
/// Case: the user presses `Ctrl+B` in vi mode.
#[test]
fn a_page_scroll_moves_the_vi_cursor_by_a_screenful() {
    let mut screen = scrolled_screen();
    let _ = screen.vi_scroll(Scroll::PageUp, &chars());
    assert_eq!(screen.display_offset(), DisplayOffset(3));
    assert_eq!(vi_point(&screen), Some(point(-1, 0)));
}

/// Asserts that `Top` and `Bottom` move the vi cursor to the first
/// non-blank cell of the oldest row and of the bottom row.
///
/// Case: the user presses `g` and then `G` in vi mode.
#[test]
fn top_and_bottom_move_the_vi_cursor_to_the_grid_ends() {
    let mut screen = scrolled_screen();
    let _ = screen.vi_scroll(Scroll::Top, &chars());
    assert_eq!(vi_point(&screen), Some(point(-3, 0)));
    let _ = screen.vi_scroll(Scroll::Bottom, &chars());
    assert_eq!(screen.display_offset(), DisplayOffset(0));
    assert_eq!(vi_point(&screen), Some(point(2, 0)));
}

/// Asserts that a page scroll on a screen without history moves only the
/// vi cursor and owes no damage.
///
/// Case: the user presses `Ctrl+B` in vi mode over a full-screen program.
#[test]
fn a_page_scroll_without_history_moves_only_the_vi_cursor() {
    let mut screen = screen(10, 3, 0);
    print_text(&mut screen, "a\nb\nc");
    assert!(screen.enter_vi_mode());
    assert_eq!(
        screen.vi_scroll(Scroll::PageUp, &chars()),
        ViewChange::Carried
    );
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that a line feed at the live tail carries the vi cursor up with
/// its text.
///
/// Case: the user sits in vi mode on the last output line while the
/// program prints another line.
#[test]
fn a_line_feed_carries_the_vi_cursor_with_its_text() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "a\nb\nc");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(2, 0));
    screen.line_feed();
    assert_eq!(vi_point(&screen), Some(point(1, 0)));
}

/// Asserts that a line feed at the live tail keeps a vi cursor on the top
/// row there rather than letting it leave the viewport.
///
/// Case: the user sits in vi mode on the top row while output keeps
/// scrolling the screen.
#[test]
fn a_line_feed_keeps_a_top_row_vi_cursor_on_the_top_row() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "a\nb\nc");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 0));
    screen.line_feed();
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that a line feed while the viewport is scrolled back keeps the
/// vi cursor on the text the held viewport still shows.
///
/// Case: the user browses scrollback in vi mode while a build keeps
/// printing.
#[test]
fn a_line_feed_while_scrolled_back_keeps_the_vi_cursor_on_its_text() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4");
    assert!(screen.enter_vi_mode());
    let _ = screen.scroll(Scroll::Delta(1));
    screen.vi.set(point(-1, 0));
    screen.line_feed();
    assert_eq!(screen.display_offset(), DisplayOffset(2));
    assert_eq!(vi_point(&screen), Some(point(-2, 0)));
}

/// Asserts that a line feed at the history cap, with the viewport on the
/// oldest row, keeps the vi cursor inside the viewport.
///
/// Case: the user reads the oldest retained output in vi mode while the
/// program keeps printing past the scrollback limit.
#[test]
fn a_line_feed_at_the_history_cap_keeps_the_vi_cursor_in_view() {
    let mut screen = screen(10, 3, 2);
    print_text(&mut screen, "1\n2\n3\n4\n5");
    assert!(screen.enter_vi_mode());
    let _ = screen.scroll(Scroll::Top);
    screen.vi.set(point(-2, 0));
    screen.line_feed();
    assert_eq!(screen.display_offset(), DisplayOffset(2));
    assert_eq!(vi_point(&screen), Some(point(-2, 0)));
}

/// Asserts that a region scroll moves a vi cursor inside the region and
/// leaves one above it alone.
///
/// Case: a pager with a status line scrolls its text region while the
/// user sits in vi mode.
#[test]
fn a_region_scroll_moves_only_a_vi_cursor_inside_the_region() {
    let mut inside = screen(10, 4, 10);
    print_text(&mut inside, "a\nb\nc\nd");
    inside.set_scroll_region(Some(2), Some(3));
    assert!(inside.enter_vi_mode());
    inside.vi.set(point(2, 0));
    inside.scroll_region_up(1);
    assert_eq!(vi_point(&inside), Some(point(1, 0)));

    let mut above = screen(10, 4, 10);
    print_text(&mut above, "a\nb\nc\nd");
    above.set_scroll_region(Some(2), Some(3));
    assert!(above.enter_vi_mode());
    above.vi.set(point(0, 0));
    above.scroll_region_up(1);
    assert_eq!(vi_point(&above), Some(point(0, 0)));
}

/// Asserts that a reverse index at the top pushes the vi cursor down with
/// its text and stops it at the bottom row.
///
/// Case: a full-screen program scrolls its view back one line while the
/// user sits in vi mode.
#[test]
fn a_reverse_index_pushes_the_vi_cursor_down() {
    let mut middle = screen(10, 3, 10);
    print_text(&mut middle, "a\nb\nc");
    middle.move_cursor_to(Some(1), Some(1));
    assert!(middle.enter_vi_mode());
    middle.vi.set(point(1, 0));
    middle.reverse_index();
    assert_eq!(vi_point(&middle), Some(point(2, 0)));

    let mut bottom = screen(10, 3, 10);
    print_text(&mut bottom, "a\nb\nc");
    bottom.move_cursor_to(Some(1), Some(1));
    assert!(bottom.enter_vi_mode());
    bottom.vi.set(point(2, 0));
    bottom.reverse_index();
    assert_eq!(vi_point(&bottom), Some(point(2, 0)));
}

/// Asserts that a downward region scroll while the viewport is scrolled
/// back pulls the vi cursor back inside the viewport.
///
/// Case: the user browses scrollback in vi mode with the vi cursor on the
/// bottom visible row when the program sends `SD`.
#[test]
fn a_scroll_down_while_scrolled_back_keeps_the_vi_cursor_in_view() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4\n5");
    assert!(screen.enter_vi_mode());
    let _ = screen.scroll(Scroll::Delta(1));
    screen.vi.set(point(1, 0));
    let _ = screen.scroll_region_down(1);
    assert_eq!(screen.display_offset(), DisplayOffset(1));
    assert_eq!(vi_point(&screen), Some(point(1, 0)));
}

/// Asserts that a reflow carries the vi cursor to the same character.
///
/// Case: the user narrows the window while the vi cursor sits inside a
/// line that now wraps.
#[test]
fn a_reflow_carries_the_vi_cursor_to_its_character() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "abcdefgh");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 6));
    let _ = screen.reflow(GridSize { cols: 4, rows: 3 }, ScrollbackOnGrow::Reclaim);
    assert_eq!(vi_point(&screen), Some(point(1, 2)));
}

/// Asserts that a reflow that drops the vi cursor's row seats the vi
/// cursor on the viewport's top-left cell.
///
/// Case: the user shrinks a pane without scrollback to one row while the
/// vi cursor sits on a row that no longer fits.
#[test]
fn a_reflow_that_drops_the_vi_cursor_row_seats_it_top_left() {
    let mut screen = screen(4, 3, 0);
    print_text(&mut screen, "a\nb\nc");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 1));
    let _ = screen.reflow(GridSize { cols: 4, rows: 1 }, ScrollbackOnGrow::Reclaim);
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that a truncating resize carries the vi cursor with the rows it
/// scrolls off the top.
///
/// Case: the user shrinks the window while a full-screen program is shown
/// and the vi cursor sits on its middle row.
#[test]
fn a_truncating_resize_carries_the_vi_cursor_with_its_row() {
    let mut screen = screen(4, 3, 0);
    print_text(&mut screen, "a\nb\nc");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(1, 0));
    let _ = screen.resize(GridSize { cols: 4, rows: 2 });
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that a growing resize carries the vi cursor down with the rows
/// it reclaims from history.
///
/// Case: the user makes a pane taller while the vi cursor sits on a line of
/// output.
#[test]
fn a_growing_resize_carries_the_vi_cursor_down_with_reclaimed_rows() {
    let mut screen = screen(4, 3, 10);
    print_text(&mut screen, "a\nb\nc\nd");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(1, 0));
    let _ = screen.resize(GridSize { cols: 4, rows: 4 });
    assert_eq!(vi_point(&screen), Some(point(2, 0)));
}

/// Asserts that a reflow in vi mode keeps a selection end set from the
/// right side of a cell on that cell when the rewrap starts a new row
/// right after it.
///
/// Case: in vi mode the user selects `abcd` out of `abcdefgh` and then
/// narrows the window so the line wraps right after `d`.
#[test]
fn a_reflow_keeps_a_right_side_selection_end_on_its_cell() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "abcdefgh");
    assert!(screen.enter_vi_mode());
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Right);
    let _ = screen.reflow(GridSize { cols: 4, rows: 3 }, ScrollbackOnGrow::Reclaim);
    let (anchor, moving) = screen.selection.ends().expect("an active selection");
    assert_eq!(moving.line(), anchor.line());
    assert_eq!(moving.column(), 3);
    assert_eq!(screen.selection_text().as_deref(), Some("abcd"));
}

/// Asserts that a reflow in vi mode keeps a selection end set from the
/// right side of a cell on a blank line on that blank line.
///
/// Case: in vi mode the user selects text down to an empty line and then
/// narrows the window.
#[test]
fn a_reflow_keeps_a_right_side_selection_end_on_its_blank_line() {
    let mut screen = screen(10, 4, 10);
    print_text(&mut screen, "abcdefgh\n\nxy");
    assert!(screen.enter_vi_mode());
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 2), CellSide::Right);
    let _ = screen.reflow(GridSize { cols: 4, rows: 4 }, ScrollbackOnGrow::Reclaim);
    let (_, moving) = screen.selection.ends().expect("an active selection");
    assert_eq!(Some(moving.line()), screen.grid.line_id_at(GridLine(2)));
    assert_eq!(screen.selection_text().as_deref(), Some("abcdefgh\n"));
}

/// Asserts that a reflow in vi mode keeps a selection end set from the
/// right side of a cell on that cell when the row above ends in a wrap
/// filler.
///
/// Case: in vi mode the user selects backward from `b` in `ab日xy`,
/// narrows the window so `日` no longer fits after `b`, and presses `j`.
#[test]
fn a_reflow_keeps_a_right_side_end_on_its_cell_before_a_wrap_filler() {
    let mut screen = screen(6, 3, 10);
    print_text(&mut screen, "ab日xy");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 1));
    assert!(screen.toggle_vi_selection(SelectionKind::Simple));
    let _ = screen.vi_motion(ViMotion::Left, &chars());
    let _ = screen.reflow(GridSize { cols: 3, rows: 3 }, ScrollbackOnGrow::Reclaim);
    let (anchor, _) = screen.selection.ends().expect("an active selection");
    assert_eq!(anchor.column(), 1);
    let _ = screen.vi_motion(ViMotion::Down, &chars());
    assert_eq!(screen.selection_text().as_deref(), Some("b日"));
}

/// Asserts that a copy right after a reflow in vi mode still includes a
/// blank line the selection ended on.
///
/// Case: in vi mode the user presses `v`, moves down onto an empty line,
/// narrows the window, and yanks at once.
#[test]
fn a_reflow_in_vi_mode_keeps_a_blank_end_line_selected() {
    let mut screen = screen(10, 4, 10);
    print_text(&mut screen, "abcdefgh\n\nxy");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 0));
    assert!(screen.toggle_vi_selection(SelectionKind::Simple));
    let _ = screen.vi_motion(ViMotion::Down, &chars());
    let _ = screen.reflow(GridSize { cols: 4, rows: 4 }, ScrollbackOnGrow::Reclaim);
    assert_eq!(screen.selection_text().as_deref(), Some("abcdefgh\n"));
}

/// Asserts that a blank and a tab end a semantic word whatever characters
/// the separator set was built from.
///
/// Case: the user sets `semantic_escape_chars = "-"`, which leaves the
/// blank out.
#[test]
fn whitespace_ends_a_semantic_word_in_any_separator_set() {
    let chars = SemanticEscapeChars::new("-");
    assert!(chars.contains(' '));
    assert!(chars.contains('\t'));
    assert!(chars.contains('-'));
    assert!(!chars.contains('a'));
}

/// A 10×3 screen reading `hello` on its top row, in vi mode with the vi
/// cursor on `e`.
fn hello_screen() -> Screen {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "hello");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 1));
    screen
}

/// Asserts that `v` selects the cell under the vi cursor at once.
///
/// Case: the user presses `v` on a letter and yanks without moving.
#[test]
fn v_selects_the_cell_under_the_vi_cursor() {
    let mut screen = hello_screen();
    assert!(screen.toggle_vi_selection(SelectionKind::Simple));
    assert_eq!(screen.selection_text().as_deref(), Some("e"));
}

/// Asserts that a second `v` clears the selection.
///
/// Case: the user presses `v` twice to cancel a selection.
#[test]
fn a_second_v_clears_the_selection() {
    let mut screen = hello_screen();
    screen.toggle_vi_selection(SelectionKind::Simple);
    assert!(screen.toggle_vi_selection(SelectionKind::Simple));
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that `V` after `v` switches the selection to whole lines.
///
/// Case: the user starts a character selection and widens it to lines.
#[test]
fn v_then_shift_v_switches_to_lines() {
    let mut screen = hello_screen();
    screen.toggle_vi_selection(SelectionKind::Simple);
    assert!(screen.toggle_vi_selection(SelectionKind::Lines));
    assert_eq!(screen.selection_text().as_deref(), Some("hello"));
}

/// Asserts that a toggle outside vi mode changes nothing.
///
/// Case: a toggle request arrives just after the terminal left vi mode.
#[test]
fn a_toggle_outside_vi_mode_changes_nothing() {
    let mut screen = screen(10, 3, 10);
    assert!(!screen.toggle_vi_selection(SelectionKind::Simple));
}

/// Asserts that a motion extends a selection to the vi cursor's cell,
/// inclusive.
///
/// Case: the user presses `v` and then `l` three times.
#[test]
fn a_motion_extends_the_selection_to_the_vi_cursor() {
    let mut screen = hello_screen();
    screen.toggle_vi_selection(SelectionKind::Simple);
    for _ in 0..3 {
        let _ = screen.vi_motion(ViMotion::Right, &chars());
    }
    assert_eq!(screen.selection_text().as_deref(), Some("ello"));
}

/// Asserts that a motion that cannot move the vi cursor leaves the
/// selection as it was and reports no change.
///
/// Case: the user starts a selection on the first column and presses `h`,
/// which cannot move.
#[test]
fn a_motion_that_cannot_move_leaves_the_selection_unchanged() {
    let mut screen = hello_screen();
    screen.vi.set(point(0, 0));
    screen.toggle_vi_selection(SelectionKind::Simple);
    assert_eq!(
        screen.vi_motion(ViMotion::Left, &chars()),
        ViewChange::Unchanged
    );
    assert_eq!(screen.selection_text().as_deref(), Some("h"));
}

/// Asserts that a backward motion keeps the anchor's cell in the
/// selection.
///
/// Case: the user presses `v` on the second `l` and then `h` twice.
#[test]
fn a_backward_motion_keeps_the_anchor_cell() {
    let mut screen = hello_screen();
    screen.vi.set(point(0, 3));
    screen.toggle_vi_selection(SelectionKind::Simple);
    for _ in 0..2 {
        let _ = screen.vi_motion(ViMotion::Left, &chars());
    }
    assert_eq!(screen.selection_text().as_deref(), Some("ell"));
}

/// Asserts that an empty selection does not follow the vi cursor.
///
/// Case: the user clicks once in vi mode, leaving an empty selection, and
/// then moves with `l`.
#[test]
fn an_empty_selection_does_not_follow_the_vi_cursor() {
    let mut screen = hello_screen();
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    let _ = screen.vi_motion(ViMotion::Right, &chars());
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that a page scroll extends a selection to the moved vi cursor.
///
/// Case: the user presses `v` on the last output line and then `Ctrl+B`.
#[test]
fn a_page_scroll_extends_the_selection() {
    let mut screen = scrolled_screen();
    screen.vi.set(point(2, 0));
    screen.toggle_vi_selection(SelectionKind::Simple);
    let _ = screen.vi_scroll(Scroll::PageUp, &chars());
    assert_eq!(screen.selection_text().as_deref(), Some("3\n4\n5\n6"));
}

/// Asserts that a click in vi mode moves the vi cursor to the clicked
/// cell.
///
/// Case: the user clicks a word in vi mode to jump there.
#[test]
fn a_click_in_vi_mode_moves_the_vi_cursor() {
    let mut screen = hello_screen();
    assert!(screen.start_selection(point(0, 3), CellSide::Left, SelectionKind::Simple));
    assert_eq!(vi_point(&screen), Some(point(0, 3)));
}

/// Asserts that a drag in vi mode moves the vi cursor to the dragged cell
/// and covers both end cells.
///
/// Case: the user drags across `ell` in vi mode.
#[test]
fn a_drag_in_vi_mode_covers_both_end_cells() {
    let mut screen = hello_screen();
    screen.start_selection(point(0, 1), CellSide::Left, SelectionKind::Simple);
    assert!(screen.extend_selection(point(0, 3), CellSide::Left));
    assert_eq!(vi_point(&screen), Some(point(0, 3)));
    assert_eq!(screen.selection_text().as_deref(), Some("ell"));
}

/// Asserts that a drag outside vi mode keeps the boundary it was given and
/// leaves no vi cursor.
///
/// Case: the user drags across `el` without vi mode.
#[test]
fn a_drag_outside_vi_mode_is_unchanged() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "hello");
    screen.start_selection(point(0, 1), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Left);
    assert_eq!(screen.selection_text().as_deref(), Some("el"));
    assert_eq!(screen.vi_cursor(), None);
}

/// Asserts that an extend without a selection leaves the vi cursor where
/// it was.
///
/// Case: a stray drag event arrives in vi mode after the selection was
/// cleared.
#[test]
fn an_extend_without_a_selection_leaves_the_vi_cursor() {
    let mut screen = hello_screen();
    assert!(!screen.extend_selection(point(0, 3), CellSide::Left));
    assert_eq!(vi_point(&screen), Some(point(0, 1)));
}
