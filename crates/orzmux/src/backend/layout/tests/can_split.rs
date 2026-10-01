//! Tests for `Tiling::can_split`: the room a pane needs before it
//! divides.

use super::*;

/// Asserts that a pane can split along an axis only when that axis
/// holds two minimum leaves and a separator: five columns for a
/// vertical split, three rows for a horizontal one.
///
/// Case: the user keeps splitting a pane until it is too small to
/// divide again.
#[test]
fn a_pane_needs_room_for_two_minimum_leaves_and_a_separator() {
    let tree = LayoutTree::with_root(PaneId(1));
    let can_split = |cols, rows, orientation| {
        tree.tile(GridSize { cols, rows })
            .can_split(PaneId(1), orientation)
    };
    assert!(can_split(5, 1, SplitOrientation::Vertical));
    assert!(!can_split(4, 24, SplitOrientation::Vertical));
    assert!(can_split(2, 3, SplitOrientation::Horizontal));
    assert!(!can_split(80, 2, SplitOrientation::Horizontal));
}

/// Asserts that the room is judged on the pane's own rectangle rather
/// than on the window.
///
/// Case: two panes sit side by side in a nine-column window, and the
/// user asks to split the right one.
#[test]
fn room_is_judged_on_the_panes_own_rectangle() {
    let mut ids = SplitIds::default();
    let tree = two_side_by_side(&mut ids);
    let narrow = GridSize { cols: 9, rows: 24 };
    assert!(
        !tree
            .tile(narrow)
            .can_split(PaneId(2), SplitOrientation::Vertical)
    );
    assert!(
        tree.tile(narrow)
            .can_split(PaneId(2), SplitOrientation::Horizontal)
    );
    assert!(
        tree.tile(W)
            .can_split(PaneId(2), SplitOrientation::Vertical)
    );
}

/// Asserts that a pane the tiling does not hold cannot be split.
///
/// Case: a split request names a pane of another tab.
#[test]
fn an_absent_pane_cannot_be_split() {
    let mut ids = SplitIds::default();
    let tree = two_side_by_side(&mut ids);
    assert!(
        !tree
            .tile(W)
            .can_split(PaneId(9), SplitOrientation::Vertical)
    );
}
