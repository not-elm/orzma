//! Tests for `select_direction`: adjacency, the recency tie-break, and
//! clipped layouts.

use super::*;

/// Asserts that directional selection picks the adjacent, overlapping
/// neighbour and does nothing at the window edge.
///
/// Case: the user presses select-right-pane from the left pane and
/// then again from the right pane.
#[test]
fn select_direction_moves_to_the_adjacent_pane_and_stops_at_the_edge() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.select(PaneId(1));
    assert!(tree.select_direction(PaneDirection::Right, W));
    assert_eq!(tree.active(), PaneId(2));
    assert!(!tree.select_direction(PaneDirection::Right, W));
    assert_eq!(tree.active(), PaneId(2));
    assert!(tree.select_direction(PaneDirection::Left, W));
    assert_eq!(tree.active(), PaneId(1));
}

/// Asserts that among several adjacent candidates the most recently
/// active one wins, and that never-visited ties fall to the topmost.
///
/// Case: the right column is split into top and bottom; the user
/// visited the bottom one, went left, and presses select-right again.
#[test]
fn select_direction_prefers_the_most_recently_active_candidate() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.split(
        &mut ids,
        PaneId(2),
        SplitOrientation::Horizontal,
        PaneId(3),
        W,
    )
    .unwrap();
    tree.select(PaneId(1));
    assert!(tree.select_direction(PaneDirection::Right, W));
    assert_eq!(tree.active(), PaneId(3), "pane 3 was active most recently");

    let mut fresh_ids = SplitIds::default();
    let mut fresh = two_side_by_side(&mut fresh_ids);
    fresh
        .split(
            &mut fresh_ids,
            PaneId(2),
            SplitOrientation::Horizontal,
            PaneId(3),
            W,
        )
        .unwrap();
    fresh.clear_history_for_test();
    fresh.select(PaneId(1));
    fresh.clear_history_for_test();
    assert!(fresh.select_direction(PaneDirection::Right, W));
    assert_eq!(
        fresh.active(),
        PaneId(2),
        "never-visited candidates tie to the topmost"
    );
}

/// Asserts that directional selection in a clipped layout still walks
/// to the adjacent pane rather than jumping to a farther one.
///
/// Case: three panes side by side in a window narrower than the tree;
/// the user presses select-right from the leftmost.
#[test]
fn select_direction_does_not_skip_neighbours_when_clipped() {
    let mut ids = SplitIds::default();
    let mut tree = two_side_by_side(&mut ids);
    tree.split(
        &mut ids,
        PaneId(2),
        SplitOrientation::Vertical,
        PaneId(3),
        W,
    )
    .unwrap();
    tree.select(PaneId(1));
    let tiny = GridSize { cols: 2, rows: 1 };
    assert!(tree.select_direction(PaneDirection::Right, tiny));
    assert_eq!(tree.active(), PaneId(2));
}
