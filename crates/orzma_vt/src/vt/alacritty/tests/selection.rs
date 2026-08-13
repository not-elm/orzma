//! Basic selection-operation tests: anchor, drag, clear, and boundary
//! inclusion.

use super::*;

/// Asserts that a one-cell `StartAt` yields a renderable, non-empty
/// selection with `Linear` geometry.
///
/// Case: a mouse press followed by copy. A bare `Selection::new` is
/// empty when both ends coincide, so the implementation must apply
/// the opposite-side update recipe or a click-then-copy yields
/// nothing. Also pins the Simple → Linear geometry arm.
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

/// Asserts that `UpdateTo` moves only the moving end; the anchor
/// stays where `StartAt` put it.
///
/// Case: the basic drag — press on the first cell, drag right across
/// four more. The extracted text must cover the whole span.
#[test]
fn update_to_extends_the_moving_end() {
    let mut vt = vt_after(b"abcdefghij");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 4, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("abcde"));
}

/// Asserts that `UpdateTo` with no active selection changes nothing.
///
/// Case: alacritty wipes the selection on an alt-screen swap while
/// the input glue may still deliver one more drag event; the stray
/// update must neither panic nor conjure a selection.
#[test]
fn update_to_without_a_selection_is_a_no_op() {
    let mut vt = vt_after(b"abc");
    update_to(&mut vt, 2, 0, CellSide::Right);
    assert_eq!(vt.selection_range(), None);
}

/// Asserts that the end-cell side decides whether the cell under the
/// pointer is included.
///
/// Case: the CellSide → alacritty `Side` mapping is a two-arm match;
/// a transposition compiles cleanly and off-by-ones every selection
/// the user ever drags.
#[test]
fn cell_side_decides_inclusion_of_the_boundary_cells() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 3, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("abcd"));
    vt.apply_selection(SelectionOp::Clear).unwrap();
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 3, 0, CellSide::Left);
    assert_eq!(vt.selected_text().as_deref(), Some("abc"));
}

/// Asserts that a drag toward the top-left reports a normalized
/// range with `start` at the top.
///
/// Case: an upward drag. `SelectionRange`'s doc pins start as the
/// top-left of the selected cells; a renderer given anchor-order
/// endpoints would rasterize a negative-height span.
#[test]
fn a_backward_drag_normalizes_start_before_end() {
    let mut vt = vt_after(b"one\r\ntwo\r\nthree");
    start_simple(&mut vt, 5, 2);
    update_to(&mut vt, 1, 0, CellSide::Left);
    let range = vt.selection_range().expect("backward drag must render");
    assert_eq!(range.start.row, 0);
    assert_eq!(range.end.row, 2);
}

/// Asserts that `Clear` drops the selection AND the stored anchor a
/// later `ChangeKind` would rebuild from.
///
/// Case: clear, then press `V`. An implementation keeping the saved
/// anchor would resurrect a zombie selection from pre-clear state
/// instead of treating the change as a no-op.
#[test]
fn clear_discards_the_selection_and_the_stored_anchor() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.selection_range(), None);
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_range(), None, "no zombie from a stale anchor");
}

/// Asserts that dragging back onto the anchor cell/side makes the
/// selection empty for `selection_range` and `selected_text` while
/// `selection_kind` still reports the live selection object.
///
/// Case: a drag that returns to its starting point. The trait doc
/// says range/text are `None` for an empty selection but kind is
/// `None` only when NO selection exists — this pins the three
/// getters to one consistent notion of "empty".
#[test]
fn an_update_back_onto_the_anchor_empties_the_selection() {
    let mut vt = vt_after(b"abc");
    start_simple(&mut vt, 1, 0);
    update_to(&mut vt, 1, 0, CellSide::Left);
    assert_eq!(vt.selection_range(), None);
    assert_eq!(vt.selected_text(), None);
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Simple));
}
