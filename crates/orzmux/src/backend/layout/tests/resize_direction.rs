//! Tests for `resize_direction`, the keyboard resize: which divider a pane
//! moves, and where the divider stops.

use super::*;

/// Builds `A | (B | C)` in an 80×24 window with C active: the root
/// divider sits at x = 40 and the B|C divider at x = 60.
fn three_columns(ids: &mut SplitIds) -> LayoutTree {
    let mut tree = two_side_by_side(ids);
    tree.split(ids, PaneId(2), SplitOrientation::Vertical, PaneId(3))
        .unwrap();
    tree
}

/// Builds `(A | A') | B` in an 80×24 window by splitting A after B,
/// with A' active: the A|A' divider sits at x = 20 and the root
/// divider at x = 40.
fn nested_run(ids: &mut SplitIds) -> LayoutTree {
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(ids, PaneId(1), SplitOrientation::Vertical, PaneId(3))
        .unwrap();
    tree.split(ids, PaneId(1), SplitOrientation::Vertical, PaneId(2))
        .unwrap();
    tree
}

/// Asserts that a pane with a divider after it moves that divider in
/// the key's direction by the requested cells.
///
/// Case: the user nudges the right border of the middle one of three
/// side-by-side panes to the left, then back to the right.
#[test]
fn a_middle_pane_moves_its_right_border() {
    let mut ids = SplitIds::default();
    let mut tree = three_columns(&mut ids);
    tree.select(PaneId(2));

    assert!(tree.resize_direction(PaneDirection::Left, 5, W));
    let tiling = tree.tile(W);
    assert_eq!(tiling.separators[0].x, 40);
    assert_eq!(tiling.separators[1].x, 55);

    assert!(tree.resize_direction(PaneDirection::Right, 5, W));
    assert_eq!(tree.tile(W).separators[1].x, 60);
}

/// Asserts that the last pane of a row, which has no divider after
/// it, moves the divider before it.
///
/// Case: the user presses resize-left-pane in the rightmost of three
/// side-by-side panes to widen it.
#[test]
fn the_last_pane_of_a_row_moves_its_left_border() {
    let mut ids = SplitIds::default();
    let mut tree = three_columns(&mut ids);

    assert!(tree.resize_direction(PaneDirection::Left, 5, W));

    let tiling = tree.tile(W);
    assert_eq!(tiling.separators[0].x, 40);
    assert_eq!(tiling.separators[1].x, 55);
    assert_eq!(rect_of(&tiling, PaneId(3)).cols, 24);
}

/// Asserts that up and down move the divider of a stacked pair: the
/// top pane moves the divider below it and the bottom pane the
/// divider above it.
///
/// Case: the user presses resize-down-pane in the top pane, then
/// resize-up-pane twice in the bottom pane.
#[test]
fn stacked_panes_move_the_horizontal_divider() {
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(&mut ids, PaneId(1), SplitOrientation::Horizontal, PaneId(2))
        .unwrap();
    tree.select(PaneId(1));

    assert!(tree.resize_direction(PaneDirection::Down, 5, W));
    assert_eq!(tree.tile(W).separators[0].y, 17);

    tree.select(PaneId(2));
    assert!(tree.resize_direction(PaneDirection::Up, 5, W));
    assert!(tree.resize_direction(PaneDirection::Up, 5, W));
    assert_eq!(tree.tile(W).separators[0].y, 7);
}

/// Asserts that a pane whose next divider belongs to an outer split
/// of the same orientation moves that outer divider, and that the
/// inner split keeps its ratio as it widens.
///
/// Case: the user split the left of two panes, then presses
/// resize-right-pane in the new middle pane.
#[test]
fn a_nested_pane_moves_the_outer_divider_after_it() {
    let mut ids = SplitIds::default();
    let mut tree = nested_run(&mut ids);

    assert!(tree.resize_direction(PaneDirection::Right, 5, W));

    let tiling = tree.tile(W);
    assert_eq!(tiling.separators[1].x, 45);
    assert_eq!(tiling.separators[0].x, 22);
}

/// Asserts that the first pane of a nested run moves the nearest
/// divider after it rather than an outer one.
///
/// Case: the user presses resize-right-pane in the leftmost of three
/// panes whose left two came from splitting one pane.
#[test]
fn the_first_pane_of_a_nested_run_moves_the_nearest_divider() {
    let mut ids = SplitIds::default();
    let mut tree = nested_run(&mut ids);
    tree.select(PaneId(1));

    assert!(tree.resize_direction(PaneDirection::Right, 5, W));

    let tiling = tree.tile(W);
    assert_eq!(tiling.separators[0].x, 25);
    assert_eq!(tiling.separators[1].x, 40);
}

/// Asserts that the search for a divider crosses a split of the
/// other orientation to reach the nearest one on the key's axis.
///
/// Case: the right column is split into a top and a bottom pane, and
/// the user presses resize-left-pane in the bottom one.
#[test]
fn a_stacked_pane_moves_the_column_divider_beside_it() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.split(&mut ids, PaneId(2), SplitOrientation::Horizontal, PaneId(3))
        .unwrap();

    assert!(tree.resize_direction(PaneDirection::Left, 5, W));

    assert_eq!(tree.tile(W).separators[0].x, 35);
}

/// Asserts that the run of same-orientation splits stops at a split
/// of the other orientation, so the pane moves the divider inside
/// its own row rather than the outer divider on its right.
///
/// Case: the left column holds A above a B|C row, D sits to the
/// right, and the user presses resize-left-pane in C.
#[test]
fn the_run_stops_at_a_split_of_the_other_orientation() {
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(&mut ids, PaneId(1), SplitOrientation::Vertical, PaneId(4))
        .unwrap();
    tree.split(&mut ids, PaneId(1), SplitOrientation::Horizontal, PaneId(2))
        .unwrap();
    tree.split(&mut ids, PaneId(2), SplitOrientation::Vertical, PaneId(3))
        .unwrap();

    assert!(tree.resize_direction(PaneDirection::Left, 5, W));

    let tiling = tree.tile(W);
    assert_eq!(tiling.separators[1].x, 15);
    assert_eq!(tiling.separators[2].x, 40);
}

/// Asserts that a resize with no divider on the key's axis, or with a
/// lone pane, is refused and changes nothing.
///
/// Case: the user presses resize-up-pane with only side-by-side panes
/// open, then resize-left-pane with a single pane.
#[test]
fn a_resize_without_a_divider_on_the_axis_is_refused() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    let before = tree.tile(W);
    assert!(!tree.resize_direction(PaneDirection::Up, 5, W));
    assert_eq!(tree.tile(W), before);

    let mut single = LayoutTree::with_root(PaneId(1));
    assert!(!single.resize_direction(PaneDirection::Left, 5, W));
}

/// Asserts that repeated presses stop the divider at the drag
/// minimum, the last step moving only as far as the minimum allows,
/// and that a press at the minimum is refused.
///
/// Case: the user holds resize-left-pane until the left pane cannot
/// shrink any further.
#[test]
fn repeated_resizes_stop_at_the_drag_minimum() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);

    for expected in [35, 30, 25, 20, 15, 10, 5, 4] {
        assert!(tree.resize_direction(PaneDirection::Left, 5, W));
        assert_eq!(tree.tile(W).separators[0].x, expected);
    }
    assert!(!tree.resize_direction(PaneDirection::Left, 5, W));
    assert_eq!(tree.tile(W).separators[0].x, 4);
}

/// Asserts that a press whose clamp would move the divider against
/// the key's direction is refused, while the opposite key still
/// moves it.
///
/// Case: the left pane is two columns wide, narrower than the drag
/// minimum, and the user presses resize-left-pane and then
/// resize-right-pane.
#[test]
fn a_resize_never_moves_the_divider_against_the_key() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.set_root_ratio_for_test(0.03);
    assert_eq!(tree.tile(W).separators[0].x, 2);

    assert!(!tree.resize_direction(PaneDirection::Left, 5, W));
    assert_eq!(tree.tile(W).separators[0].x, 2);

    assert!(tree.resize_direction(PaneDirection::Right, 5, W));
    assert_eq!(tree.tile(W).separators[0].x, 7);
}

/// Asserts that a press widening a side already narrower than the drag
/// minimum moves the divider exactly the requested cells.
///
/// Case: the right pane has been squeezed to two columns and the user
/// presses resize-left-pane to widen it.
#[test]
fn a_resize_widening_a_squeezed_side_moves_exactly_the_requested_cells() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.set_root_ratio_for_test(0.98);
    assert_eq!(tree.tile(W).separators[0].x, 77);

    assert!(tree.resize_direction(PaneDirection::Left, 5, W));

    assert_eq!(tree.tile(W).separators[0].x, 72);
}

/// Asserts that a zero-cell resize is refused.
///
/// Case: a caller sends a directional resize with a zero step.
#[test]
fn a_zero_cell_resize_is_refused() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);

    assert!(!tree.resize_direction(PaneDirection::Left, 0, W));
}

/// Asserts that in a window too small for the drag minimum the
/// divider still moves, stopping at the tree's own leaf minimum.
///
/// Case: the user shrinks the window until two side-by-side panes no
/// longer fit their drag minimum, then presses resize-left-pane.
#[test]
fn a_cramped_window_resizes_down_to_the_tree_minimum() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    let narrow = GridSize { cols: 8, rows: 24 };

    assert!(tree.resize_direction(PaneDirection::Left, 5, narrow));

    assert_eq!(tree.tile(narrow).separators[0].x, 2);
}
