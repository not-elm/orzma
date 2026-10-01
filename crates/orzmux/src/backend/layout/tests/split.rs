//! Tests for building a tree with `with_root` and `split`: the geometry a
//! split yields, the room it needs, and the ids it mints.

use super::*;

/// Asserts that a root-only tree holds exactly its pane, which is
/// active.
///
/// Case: a new tab opens with its first shell.
#[test]
fn a_root_only_tree_holds_its_active_pane() {
    let tree = LayoutTree::with_root(PaneId(7));
    assert_eq!(tree.panes(), vec![PaneId(7)]);
    assert_eq!(tree.active(), PaneId(7));
    assert!(tree.contains(PaneId(7)));
}

/// Asserts that a vertical split halves the width around a one-cell
/// separator and makes the new pane active.
///
/// Case: the user presses split-vertical-pane on the only pane of an
/// 80×24 window.
#[test]
fn a_vertical_split_halves_the_width_around_a_separator() {
    let mut ids = SplitIds::default();
    let tree = two_side_by_side(&mut ids);
    let solved = tree.solve(W);
    assert_eq!(
        rect_of(&solved, PaneId(1)),
        PaneRect {
            pane: PaneId(1),
            x: 0,
            y: 0,
            cols: 40,
            rows: 24
        }
    );
    assert_eq!(
        rect_of(&solved, PaneId(2)),
        PaneRect {
            pane: PaneId(2),
            x: 41,
            y: 0,
            cols: 39,
            rows: 24
        }
    );
    assert_eq!(
        solved.separators,
        vec![Separator {
            split: SplitId(0),
            orientation: SplitOrientation::Vertical,
            x: 40,
            y: 0,
            len: 24
        }]
    );
    assert_eq!(tree.active(), PaneId(2));
}

/// Asserts that a horizontal split stacks the panes with the new one
/// below.
///
/// Case: the user presses split-horizontal-pane on an 80×24 pane.
#[test]
fn a_horizontal_split_stacks_the_new_pane_below() {
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
    let solved = tree.solve(W);
    assert_eq!(rect_of(&solved, PaneId(1)).rows, 12);
    assert_eq!(
        rect_of(&solved, PaneId(2)),
        PaneRect {
            pane: PaneId(2),
            x: 0,
            y: 13,
            cols: 80,
            rows: 11
        }
    );
}

/// Asserts that a leaf too narrow to hold two minimum leaves and a
/// separator along the split axis refuses to split.
///
/// Case: the user keeps splitting a pane until it is two columns wide.
#[test]
fn a_split_needs_room_for_two_minimum_leaves_and_a_separator() {
    let mut ids = SplitIds::default();
    let mut tree = LayoutTree::with_root(PaneId(1));
    let narrow = GridSize { cols: 2, rows: 24 };
    assert!(
        tree.split(
            &mut ids,
            PaneId(1),
            SplitOrientation::Vertical,
            PaneId(2),
            narrow
        )
        .is_err()
    );
    assert!(
        tree.split(
            &mut ids,
            PaneId(1),
            SplitOrientation::Horizontal,
            PaneId(2),
            narrow
        )
        .is_ok()
    );
}

/// Asserts that two trees splitting through one `SplitIds` never
/// hand out the same split id.
///
/// Case: two tabs each split their first pane, and the GUI keys
/// its divider nodes by split id.
#[test]
fn trees_sharing_split_ids_never_reuse_an_id() {
    let mut ids = SplitIds::default();
    let mut a = LayoutTree::with_root(PaneId(1));
    let mut b = LayoutTree::with_root(PaneId(3));
    a.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(2),
        W,
    )
    .expect("an 80-column pane splits");
    b.split(
        &mut ids,
        PaneId(3),
        SplitOrientation::Vertical,
        PaneId(4),
        W,
    )
    .expect("an 80-column pane splits");
    assert_ne!(
        a.solve(W).separators[0].split,
        b.solve(W).separators[0].split
    );
}

/// Asserts that the id of a removed split is never handed to a later
/// split.
///
/// Case: the user splits a pane, closes the new pane, then splits
/// again.
#[test]
fn a_split_id_is_never_reused() {
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
    let first = tree.solve(W).separators[0].split;

    let mut tree = removed(tree, PaneId(2));
    assert!(tree.solve(W).separators.is_empty());

    tree.split(
        &mut ids,
        PaneId(1),
        SplitOrientation::Vertical,
        PaneId(3),
        W,
    )
    .unwrap();
    assert_ne!(tree.solve(W).separators[0].split, first);
}
