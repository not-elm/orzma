//! Tests for `resize_split`, the divider drag: the cell it lands on and
//! the minimums it clamps to.

use super::*;

/// Asserts that the drag minimum of a column of N panes is
/// `5N - 1` cells wide and that stacking N panes needs `3N - 1`
/// rows, so the per-leaf minimum composes through nested splits.
///
/// Case: the user has built a three-pane column and then a
/// three-pane stack.
#[test]
fn the_drag_minimum_composes_through_nested_splits() {
    let mut column_ids = SplitIds::default();
    let mut columns = LayoutTree::with_root(PaneId(1));
    columns
        .split(
            &mut column_ids,
            PaneId(1),
            SplitOrientation::Vertical,
            PaneId(2),
            W,
        )
        .unwrap();
    columns
        .split(
            &mut column_ids,
            PaneId(2),
            SplitOrientation::Vertical,
            PaneId(3),
            W,
        )
        .unwrap();
    assert_eq!(columns.min_size_for_drag().cols, 14);

    let mut row_ids = SplitIds::default();
    let mut rows = LayoutTree::with_root(PaneId(1));
    rows.split(
        &mut row_ids,
        PaneId(1),
        SplitOrientation::Horizontal,
        PaneId(2),
        W,
    )
    .unwrap();
    rows.split(
        &mut row_ids,
        PaneId(2),
        SplitOrientation::Horizontal,
        PaneId(3),
        W,
    )
    .unwrap();
    assert_eq!(rows.min_size_for_drag().rows, 8);
}

/// Asserts that moving a divider puts it on the requested
/// whole-window cell and that solving again reports it there, so the
/// cell → ratio → cell round trip is exact.
///
/// Case: the user drags the divider of a two-pane 80×24 window from
/// the middle out to column 60.
#[test]
fn a_resize_puts_the_divider_on_the_requested_cell() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    let split = tree.solve(W).separators[0].split;

    assert!(tree.resize_split(split, 60, W));

    let solved = tree.solve(W);
    assert_eq!(solved.separators[0].x, 60);
    assert_eq!(rect_of(&solved, PaneId(1)).cols, 60);
    assert_eq!(rect_of(&solved, PaneId(2)).cols, 19);
}

/// Asserts that a divider dragged past either end stops at the drag
/// minimum rather than collapsing the pane behind it.
///
/// Case: the user throws the divider of a two-pane 80×24 window all
/// the way to the left edge, then all the way to the right.
#[test]
fn a_resize_clamps_to_the_drag_minimum_on_both_sides() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    let split = tree.solve(W).separators[0].split;

    assert!(tree.resize_split(split, 0, W));
    assert_eq!(tree.solve(W).separators[0].x, 4);

    assert!(tree.resize_split(split, 79, W));
    assert_eq!(tree.solve(W).separators[0].x, 75);
}

/// Asserts that a divider dragged past either end of a stacked pair
/// stops at the drag minimum in rows rather than in columns.
///
/// Case: the user throws the divider of a two-pane 80×24 window up
/// to the top edge, then all the way down to the bottom.
#[test]
fn a_horizontal_resize_clamps_to_the_drag_minimum_on_both_sides() {
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Horizontal,
        PaneId(2),
        W,
    )
    .unwrap();
    let split = tree.solve(W).separators[0].split;

    assert!(tree.resize_split(split, 0, W));
    assert_eq!(tree.solve(W).separators[0].y, 2);

    assert!(tree.resize_split(split, 23, W));
    assert_eq!(tree.solve(W).separators[0].y, 21);
}

/// Asserts that a resize addressed to an id no longer in the tree is
/// refused and changes nothing.
///
/// Case: the shell in one pane exits mid-drag, collapsing the split
/// the pointer was holding, and the next drag frame still names it.
#[test]
fn a_resize_of_an_unknown_split_is_refused() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    let before = tree.solve(W);

    assert!(!tree.resize_split(SplitId(999), 60, W));

    assert_eq!(tree.solve(W), before);
}

/// Asserts that a position left of the split's own origin saturates
/// to the drag minimum instead of underflowing.
///
/// Case: the user drags the right-hand divider of a nested layout
/// far past the left edge of the window.
#[test]
fn a_position_before_the_split_origin_saturates() {
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(2),
        W,
    )
    .unwrap();
    tree.split(
        &mut ids,
        PaneId(2),
        SplitOrientation::Vertical,
        PaneId(3),
        W,
    )
    .unwrap();
    let inner = tree.solve(W).separators[1].split;

    assert!(tree.resize_split(inner, 0, W));

    let solved = tree.solve(W);
    assert_eq!(solved.separators[1].x, 45);
}

/// Asserts that resizing an inner split leaves the outer divider
/// where it was.
///
/// Case: the user drags the divider inside the right-hand column of
/// a two-column layout.
#[test]
fn resizing_an_inner_split_leaves_the_outer_divider() {
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(2),
        W,
    )
    .unwrap();
    tree.split(
        &mut ids,
        PaneId(2),
        SplitOrientation::Horizontal,
        PaneId(3),
        W,
    )
    .unwrap();
    let outer_x = tree.solve(W).separators[0].x;
    let inner = tree.solve(W).separators[1].split;

    assert!(tree.resize_split(inner, 6, W));

    let solved = tree.solve(W);
    assert_eq!(solved.separators[0].x, outer_x);
    assert_eq!(solved.separators[1].y, 6);
}

/// Asserts that a window too narrow to honour the drag minimum on
/// both sides still lets the divider move, falling back to the
/// tree's own two-column leaf minimum.
///
/// Case: the user shrinks the window until a two-pane row no
/// longer fits its drag minimum, then drags a divider anyway.
#[test]
fn a_cramped_window_falls_back_to_the_tree_minimum() {
    let narrow = GridSize { cols: 8, rows: 24 };
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(2),
        narrow,
    )
    .unwrap();
    let split = tree.solve(narrow).separators[0].split;

    assert!(tree.resize_split(split, 1, narrow));

    assert_eq!(tree.solve(narrow).separators[0].x, 2);
}

/// Asserts that resizing against a window smaller than the one the
/// layout was built for neither panics nor leaves the divider
/// outside the solved extent.
///
/// Case: the user drags a divider while dragging the window's own
/// resize corner, so a smaller window arrives mid-drag.
#[test]
fn a_resize_against_a_shrunken_window_is_clamped() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    let split = tree.solve(W).separators[0].split;
    let shrunk = GridSize { cols: 20, rows: 10 };

    assert!(tree.resize_split(split, 18, shrunk));

    let solved = tree.solve(shrunk);
    assert_eq!(solved.separators[0].x, 15);
}

/// Asserts that a column deeper than the window's drag minimum still
/// moves its divider instead of panicking.
///
/// Case: the user has four columns open and shrinks the window until
/// the per-leaf drag minimum no longer fits, then drags a divider.
#[test]
fn a_deep_column_below_the_drag_minimum_still_resizes() {
    let wide = GridSize { cols: 23, rows: 24 };
    let narrow = GridSize { cols: 16, rows: 24 };
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    for (target, new) in [(1, 2), (2, 3), (3, 4)] {
        tree.split(
            &mut ids,
            PaneId(target),
            SplitOrientation::Vertical,
            PaneId(new),
            wide,
        )
        .unwrap();
    }
    let split = tree.solve(narrow).separators[0].split;

    assert!(tree.resize_split(split, 14, narrow));

    let solved = tree.solve(narrow);
    assert_eq!(solved.separators[0].x, 7);
}
