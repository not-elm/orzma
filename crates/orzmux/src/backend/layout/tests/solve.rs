//! Tests for `solve` against a window smaller than the tree's minimum.

use super::*;

/// Asserts that a window smaller than the tree's minimum solves
/// against the minimum, keeping every pane at its leaf minimum and
/// overflowing the window instead of collapsing.
///
/// Case: the user shrinks the window to one column while two panes
/// sit side by side.
#[test]
fn a_window_below_the_minimum_solves_as_a_clipped_virtual_layout() {
    let mut ids = SplitIds::default();
    let tree = two_side_by_side(&mut ids);
    let solved = tree.solve(GridSize { cols: 1, rows: 1 });
    assert_eq!(
        rect_of(&solved, PaneId(1)),
        PaneRect {
            pane: PaneId(1),
            x: 0,
            y: 0,
            cols: 2,
            rows: 1
        }
    );
    assert_eq!(
        rect_of(&solved, PaneId(2)),
        PaneRect {
            pane: PaneId(2),
            x: 3,
            y: 0,
            cols: 2,
            rows: 1
        }
    );
}
