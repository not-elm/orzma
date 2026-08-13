//! Basic selection-operation tests: anchor, drag, clear, and boundary
//! inclusion.

use super::*;

/// Asserts that a one-cell `start_selection` yields a renderable,
/// non-empty selection with `Linear` geometry.
///
/// Case: the user clicks a single cell and copies it.
#[test]
fn start_at_anchors_a_non_empty_selection() {
    let mut vt = vt_after(b"hi");
    start_simple(&mut vt, 0, 0);
    let range = vt.selection_range().expect("one-cell start must render");
    assert_eq!(range.start, ViewportPoint { row: 0, column: 0 });
    assert_eq!(range.end, ViewportPoint { row: 0, column: 0 });
    assert_eq!(range.geometry, SelectionGeometry::Linear);
    assert_eq!(vt.selected_text().as_deref(), Some("h"));
}

/// Asserts that `update_selection` moves only the moving end; the
/// anchor stays where `start_selection` put it.
///
/// Case: the basic drag — the user presses on the first cell and
/// drags right across four more.
#[test]
fn update_to_extends_the_moving_end() {
    let mut vt = vt_after(b"abcdefghij");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 4, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("abcde"));
}

/// Asserts that `update_selection` with no active selection changes
/// nothing.
///
/// Case: an alt-screen swap wipes the selection while the input glue
/// still delivers one more drag event.
#[test]
fn update_to_without_a_selection_is_a_no_op() {
    let mut vt = vt_after(b"abc");
    update_to(&mut vt, 2, 0, CellSide::Right);
    assert_eq!(vt.selection_range(), None);
}

/// Asserts that the end-cell side decides whether the cell under the
/// pointer is included.
///
/// Case: the user drags to a boundary cell, and which half of it the
/// pointer sits in decides whether that cell is highlighted.
#[test]
fn cell_side_decides_inclusion_of_the_boundary_cells() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 3, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("abcd"));
    vt.clear_selection().unwrap();
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 3, 0, CellSide::Left);
    assert_eq!(vt.selected_text().as_deref(), Some("abc"));
}

/// Asserts that a drag toward the top-left reports a normalized
/// range with `start` at the top.
///
/// Case: the user drags upward, toward the top-left.
#[test]
fn a_backward_drag_normalizes_start_before_end() {
    let mut vt = vt_after(b"one\r\ntwo\r\nthree");
    start_simple(&mut vt, 5, 2);
    update_to(&mut vt, 1, 0, CellSide::Left);
    let range = vt.selection_range().expect("backward drag must render");
    assert_eq!(range.start.row, 0);
    assert_eq!(range.end.row, 2);
}

/// Asserts that `clear_selection` drops the selection AND the stored
/// anchor a later `change_selection_kind` would rebuild from.
///
/// Case: the user clears a selection and then presses `V`.
#[test]
fn clear_discards_the_selection_and_the_stored_anchor() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    vt.clear_selection().unwrap();
    assert_eq!(vt.selection_range(), None);
    vt.change_selection_kind(SelectionKind::Lines).unwrap();
    assert_eq!(vt.selection_range(), None, "no zombie from a stale anchor");
}

/// Asserts that dragging back onto the anchor cell/side makes the
/// selection empty for `selection_range` and `selected_text` while
/// `selection_kind` still reports the live selection object.
///
/// Case: the user's drag returns to its starting point.
#[test]
fn an_update_back_onto_the_anchor_empties_the_selection() {
    let mut vt = vt_after(b"abc");
    start_simple(&mut vt, 1, 0);
    update_to(&mut vt, 1, 0, CellSide::Left);
    assert_eq!(vt.selection_range(), None);
    assert_eq!(vt.selected_text(), None);
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Simple));
}
