//! Tests for `remove`: which panes leave the tree, who inherits the space
//! and the activity, and what the surviving splits keep.

use super::*;

/// Asserts that the tree's only pane is not removed and that the tree
/// comes back unchanged.
///
/// Case: the last shell of a tab exits, and the tab closes as a whole
/// instead of keeping an empty tree.
#[test]
fn the_only_pane_is_not_removed() {
    let tree = LayoutTree::with_root(PaneId(1));
    let Removal::Last(tree) = tree.remove(PaneId(1)) else {
        panic!("expected Last");
    };
    assert_eq!(tree.panes(), vec![PaneId(1)]);
    assert_eq!(tree.active(), PaneId(1));
}

/// Asserts that removing a pane the tree does not hold changes nothing.
///
/// Case: a stale close names a pane that already left the tab.
#[test]
fn removing_an_absent_pane_changes_nothing() {
    let mut ids = SplitIds::default();
    let tree = two_side_by_side(&mut ids);
    let Removal::Absent(tree) = tree.remove(PaneId(9)) else {
        panic!("expected Absent");
    };
    assert_eq!(tree.panes(), vec![PaneId(1), PaneId(2)]);
    assert_eq!(tree.active(), PaneId(2));
}

/// Asserts that a removed active pane with no earlier activation left
/// hands the activity to the first pane left-to-right / top-to-bottom,
/// not to its sibling.
///
/// Case: the right column is split into top and bottom, the history is
/// forgotten, and the active bottom pane's shell exits.
#[test]
fn a_removed_active_pane_without_history_falls_to_the_first_pane() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.split(
        &mut ids,
        PaneId(2),
        SplitOrientation::Horizontal,
        PaneId(3),
        W,
    )
    .expect("an 80-column pane splits");
    tree.clear_history_for_test();
    let tree = removed(tree, PaneId(3));
    assert_eq!(tree.active(), PaneId(1));
}

/// Asserts that removing a pane hands its space to its sibling and
/// re-activates the most recently active survivor.
///
/// Case: the user kills the right pane of a two-pane window after
/// having worked in the left one earlier.
#[test]
fn removing_a_pane_gives_its_space_to_the_sibling_and_restores_recency() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.select(PaneId(1));
    tree.select(PaneId(2));
    let tree = removed(tree, PaneId(2));
    assert_eq!(tree.active(), PaneId(1));
    let solved = tree.solve(W);
    assert_eq!(rect_of(&solved, PaneId(1)).cols, 80);
    assert!(solved.separators.is_empty());
}

/// Asserts that removing a leaf can change a pane that was not its
/// sibling.
///
/// Case: a width-11 window holds `(A | B) | C` with the left subtree
/// at ratio 0.1, and the user kills B.
#[test]
fn removing_a_leaf_can_resize_a_pane_outside_its_subtree() {
    let window = GridSize { cols: 11, rows: 5 };
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(3),
        window,
    )
    .unwrap();
    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(2),
        window,
    )
    .unwrap();
    tree.set_root_ratio_for_test(0.1);
    let before = tree.solve(window);
    assert_eq!(rect_of(&before, PaneId(3)).cols, 5);
    let tree = removed(tree, PaneId(2));
    let after = tree.solve(window);
    assert_eq!(rect_of(&after, PaneId(1)).cols, 2);
    assert_eq!(rect_of(&after, PaneId(3)).cols, 8);
}

/// Asserts that a split that survives the collapse of an unrelated
/// subtree keeps the id it was minted with.
///
/// Case: the user splits the window vertically, splits the right
/// half horizontally, splits that bottom pane again, then closes one
/// of the innermost panes.
#[test]
fn an_ancestor_split_keeps_its_id_when_a_descendant_collapses() {
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
    tree.split(
        &mut ids,
        PaneId(3),
        SplitOrientation::Horizontal,
        PaneId(4),
        W,
    )
    .unwrap();
    let before: Vec<SplitId> = tree.solve(W).separators.iter().map(|s| s.split).collect();
    assert_eq!(before.len(), 3);

    let tree = removed(tree, PaneId(4));

    let after: Vec<SplitId> = tree.solve(W).separators.iter().map(|s| s.split).collect();
    assert_eq!(after.len(), 2);
    for id in &after {
        assert!(before.contains(id), "{id:?} was renumbered by the collapse");
    }
}
