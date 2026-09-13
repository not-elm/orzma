//! Tests for the cell range the active selection resolves to.

use super::*;

/// Prints `あい` on the first row: bodies at columns 0 and 2,
/// continuations at 1 and 3.
fn screen_with_two_wide_glyphs() -> Screen {
    let mut screen = screen();
    for c in ['あ', 'い'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen
}

/// Asserts that a selection starting on a continuation column widens
/// to the wide body on its left.
///
/// Case: the user presses the mouse on the right half of a Japanese
/// character and drags to the end of the row.
#[test]
fn a_start_on_a_continuation_widens_to_its_body() {
    let mut screen = screen_with_two_wide_glyphs();
    screen.start_selection(point(0, 1), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Right);
    let range = screen.selection_range().expect("a selection is active");
    assert_eq!(range.start, point(0, 0));
    assert_eq!(range.end, point(0, 3));
}

/// Asserts that a selection ending on a wide body widens to the
/// continuation column on its right.
///
/// Case: the user drags from the start of the row and releases on the
/// left half of a Japanese character.
#[test]
fn an_end_on_a_wide_body_widens_to_its_continuation() {
    let mut screen = screen_with_two_wide_glyphs();
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 2), CellSide::Right);
    let range = screen.selection_range().expect("a selection is active");
    assert_eq!(range.start, point(0, 0));
    assert_eq!(range.end, point(0, 3));
}

/// Asserts that a selection already ending on a continuation column is
/// left where it is.
///
/// Case: the user releases the mouse on the right half of a Japanese
/// character, which the boundary already includes whole.
#[test]
fn an_end_on_a_continuation_is_left_alone() {
    let mut screen = screen_with_two_wide_glyphs();
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 2), CellSide::Left);
    let range = screen.selection_range().expect("a selection is active");
    assert_eq!(range.start, point(0, 0));
    assert_eq!(range.end, point(0, 1));
}

/// Asserts that a whole-line selection resolves to the full row with
/// its geometry intact.
///
/// Case: the user triple-clicks a row of Japanese text.
#[test]
fn a_lines_selection_spans_the_whole_row() {
    let mut screen = screen_with_two_wide_glyphs();
    screen.start_selection(point(0, 1), CellSide::Left, SelectionKind::Lines);
    screen.extend_selection(point(0, 2), CellSide::Right);
    let range = screen.selection_range().expect("a selection is active");
    assert_eq!(range.start, point(0, 0));
    assert_eq!(range.end, point(0, 3));
    assert_eq!(range.geometry, SelectionGeometry::Lines);
}

/// Asserts that a selection anchored on narrow text widens once the row
/// underneath is rewritten with a wide glyph.
///
/// Case: the user selects one column of a prompt, and the running
/// program then redraws that row with Japanese text.
#[test]
fn widening_follows_the_current_row_contents() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.start_selection(point(0, 1), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 1), CellSide::Right);
    let before = screen.selection_range().expect("a selection is active");
    assert_eq!((before.start, before.end), (point(0, 1), point(0, 1)));
    screen.print('あ', InsertReplaceMode::Replace, AutoWrap::Enabled);
    let after = screen
        .selection_range()
        .expect("the selection survives the redraw");
    assert_eq!((after.start, after.end), (point(0, 0), point(0, 1)));
}

/// Asserts that a selection spanning two rows widens each end against
/// its own row.
///
/// Case: the user drags from the right half of a Japanese character on
/// one row to the left half of another on the next row.
#[test]
fn each_end_widens_against_its_own_row() {
    let mut screen = screen();
    for c in ['あ', 'い', 'う', 'え'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.start_selection(point(0, 3), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 0), CellSide::Right);
    let range = screen.selection_range().expect("a selection is active");
    assert_eq!(range.start, point(0, 2));
    assert_eq!(range.end, point(1, 1));
}

/// Asserts that widening an end onto a continuation in the last column
/// stays inside the row.
///
/// Case: the user releases the mouse on the left half of a Japanese
/// character that ends the row.
#[test]
fn a_widened_end_stays_inside_the_row() {
    let mut screen = screen();
    for c in ['a', 'b', 'あ'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 2), CellSide::Right);
    let range = screen.selection_range().expect("a selection is active");
    assert_eq!(range.end, point(0, 3));
    assert!(range.end.column.0 < screen.grid_size().cols);
}
