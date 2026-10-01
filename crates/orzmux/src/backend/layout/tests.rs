//! Tests for the layout tree, one file per operation under test.

use super::*;

mod remove;
mod resize_direction;
mod resize_split;
mod select_direction;
mod solve;
mod split;

const W: GridSize = GridSize { cols: 80, rows: 24 };

fn rect_of(solved: &Solved, pane: PaneId) -> PaneRect {
    solved.rect_of(pane).expect("pane rect")
}

fn two_side_by_side(ids: &mut SplitIds) -> LayoutTree {
    let mut tree = LayoutTree::with_root(PaneId(1));
    tree.split(ids, PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
        .expect("an 80-column pane splits");
    tree
}

/// The tree after removing `pane`, which must leave other panes behind.
fn removed(tree: LayoutTree, pane: PaneId) -> LayoutTree {
    match tree.remove(pane) {
        Removal::Removed(tree) => tree,
        other => panic!("expected Removed, got {other:?}"),
    }
}
