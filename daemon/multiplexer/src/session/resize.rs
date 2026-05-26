//! Pure cell-algorithm for `resize-pane`. Walks an ancestor `Split`
//! whose orientation matches the requested axis and re-weights it in
//! integer cells per spec §7.

use crate::session::cells::{Cell, CellId, LayoutCellState, SplitOrientation};
use crate::session::direction::PaneDirection;
use crate::session::pane::PaneId;
use std::collections::HashMap;

/// Hard floor on a leaf pane's cell count along the LEFTRIGHT axis.
pub(crate) const MIN_PANE_COLS: u16 = 10;

/// Hard floor on a leaf pane's cell count along the TOPBOTTOM axis.
pub(crate) const MIN_PANE_ROWS: u16 = 3;

/// Outcome of a resize call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizePaneOutcome {
    /// At least one cell of movement was applied; broadcast.
    Applied,
    /// No matching ancestor, or shrinking subtree has zero budget.
    NoOp,
}

/// Per §7.1: each `PaneDirection` maps to a `SplitOrientation` axis
/// and a signed delta direction. The matching ancestor split's
/// `lhs_cells` is updated by `sign * applied_delta`.
fn direction_to_axis_sign(d: PaneDirection) -> (SplitOrientation, i16) {
    match d {
        PaneDirection::Right => (SplitOrientation::Horizontal, 1),
        PaneDirection::Left => (SplitOrientation::Horizontal, -1),
        PaneDirection::Down => (SplitOrientation::Vertical, 1),
        PaneDirection::Up => (SplitOrientation::Vertical, -1),
    }
}

/// Walk up the parent chain from `start_cell`. Returns the id of the
/// first ancestor `Cell::Split` whose orientation matches `axis`, or
/// `None` if no such ancestor exists (the call is a NoOp).
fn find_matching_ancestor(
    state: &LayoutCellState,
    start_cell: &CellId,
    axis: SplitOrientation,
) -> Option<CellId> {
    let mut cursor = state.get(start_cell).and_then(|c| c.parent().cloned());
    while let Some(parent_id) = cursor {
        let parent = state.get(&parent_id)?;
        match parent {
            Cell::Split(s) if s.orientation == axis => return Some(parent_id),
            _ => {
                cursor = parent.parent().cloned();
            }
        }
    }
    None
}

/// Apply §7.4: for parent axis length `p` and a same-axis split with
/// weights `lhs_w` / `rhs_w`, return `(lhs_cells, rhs_cells)`. The pair
/// sums to exactly `p`. Zero total falls back to a 50/50 split via
/// `LayoutCellState::split_ratio`.
fn split_cells(p: u16, lhs_w: f32, rhs_w: f32) -> (u16, u16) {
    let ratio = LayoutCellState::split_ratio(lhs_w, rhs_w);
    let lhs = ((p as f32 * ratio).round_ties_even() as u16).min(p);
    (lhs, p - lhs)
}

/// Walk from root to `target`, applying §7.4 at each same-axis split.
/// `axis` is the resize axis; cross-axis splits pass `p` through.
fn compute_p_at(
    state: &LayoutCellState,
    target: &CellId,
    axis: SplitOrientation,
    session_cells_on_axis: u16,
) -> u16 {
    let mut path: Vec<CellId> = Vec::new();
    let mut cursor = Some(target.clone());
    while let Some(c) = cursor {
        path.push(c.clone());
        cursor = state.get(&c).and_then(|cell| cell.parent().cloned());
    }
    path.reverse();

    let mut p = session_cells_on_axis;
    for window in path.windows(2) {
        let parent_id = &window[0];
        let child_id = &window[1];
        let Some(Cell::Split(s)) = state.get(parent_id) else {
            continue;
        };
        if s.orientation != axis {
            continue;
        }
        let (lhs, rhs) = split_cells(p, s.lhs_weight, s.rhs_weight);
        p = if child_id == &s.lhs_cell { lhs } else { rhs };
    }
    p
}

/// True iff every leaf descendant of `cell` would still satisfy
/// `min_cells` if the cell's axis length were `p`. Per §7.2.
fn satisfies_min_at(
    state: &LayoutCellState,
    cell: &CellId,
    axis: SplitOrientation,
    p: u16,
    min_cells: u16,
) -> bool {
    let Some(node) = state.get(cell) else {
        return true;
    };
    match node {
        Cell::Root(r) => satisfies_min_at(state, &r.child, axis, p, min_cells),
        Cell::Pane(_) => p >= min_cells,
        Cell::Split(s) if s.orientation == axis => {
            let (lhs, rhs) = split_cells(p, s.lhs_weight, s.rhs_weight);
            satisfies_min_at(state, &s.lhs_cell, axis, lhs, min_cells)
                && satisfies_min_at(state, &s.rhs_cell, axis, rhs, min_cells)
        }
        Cell::Split(s) => {
            // Cross-axis: each child spans the full p on `axis`.
            satisfies_min_at(state, &s.lhs_cell, axis, p, min_cells)
                && satisfies_min_at(state, &s.rhs_cell, axis, p, min_cells)
        }
    }
}

/// Maximum cells removable from `cell` (current axis length `p_sub`)
/// such that all leaves still satisfy `min_cells`. Per §7.2.
fn available_to_shrink(
    state: &LayoutCellState,
    cell: &CellId,
    axis: SplitOrientation,
    p_sub: u16,
    min_cells: u16,
    requested: u16,
) -> u16 {
    if p_sub == 0 {
        return 0;
    }
    let upper = requested.min(p_sub);
    let mut max_d = 0u16;
    for d in 1..=upper {
        if satisfies_min_at(state, cell, axis, p_sub - d, min_cells) {
            max_d = d;
        } else {
            break;
        }
    }
    max_d
}

/// Per-pane entry: resolve direction → axis/sign → matching ancestor →
/// availability → apply.
pub(crate) fn resize_split_for_pane(
    state: &mut LayoutCellState,
    pane_to_cell: &HashMap<PaneId, CellId>,
    pane: &PaneId,
    direction: PaneDirection,
    amount: u16,
    session_cols: u16,
    session_rows: u16,
) -> ResizePaneOutcome {
    let Some(leaf_id) = pane_to_cell.get(pane) else {
        return ResizePaneOutcome::NoOp;
    };
    let (axis, sign) = direction_to_axis_sign(direction);
    let Some(ancestor_id) = find_matching_ancestor(state, leaf_id, axis) else {
        return ResizePaneOutcome::NoOp;
    };

    let session_p = match axis {
        SplitOrientation::Horizontal => session_cols,
        SplitOrientation::Vertical => session_rows,
    };
    let min_cells = match axis {
        SplitOrientation::Horizontal => MIN_PANE_COLS,
        SplitOrientation::Vertical => MIN_PANE_ROWS,
    };
    let p_ancestor = compute_p_at(state, &ancestor_id, axis, session_p);

    let (current_lhs, current_rhs, lhs_cell, rhs_cell) = match state.get(&ancestor_id) {
        Some(Cell::Split(s)) => {
            let (lhs, rhs) = split_cells(p_ancestor, s.lhs_weight, s.rhs_weight);
            (lhs, rhs, s.lhs_cell.clone(), s.rhs_cell.clone())
        }
        _ => return ResizePaneOutcome::NoOp,
    };

    let (shrink_cell, shrink_p) = if sign > 0 {
        (rhs_cell, current_rhs)
    } else {
        (lhs_cell, current_lhs)
    };

    let applied = available_to_shrink(state, &shrink_cell, axis, shrink_p, min_cells, amount);
    if applied == 0 {
        return ResizePaneOutcome::NoOp;
    }

    let signed_delta = sign * applied as i16;
    let new_lhs_cells: u16 =
        ((current_lhs as i32) + (signed_delta as i32)).clamp(0, p_ancestor as i32) as u16;
    let new_rhs_cells = p_ancestor - new_lhs_cells;
    let new_lhs_w = new_lhs_cells as f32 / p_ancestor as f32;
    let new_rhs_w = new_rhs_cells as f32 / p_ancestor as f32;

    if let Some(Cell::Split(s)) = state.get_mut(&ancestor_id) {
        s.lhs_weight = new_lhs_w;
        s.rhs_weight = new_rhs_w;
    }

    ResizePaneOutcome::Applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::cells::Side;
    use crate::session::pane::activity::{Activity, ActivityId};
    use crate::session::session::{Session, SessionId};

    fn fresh_session_with_split(orientation: SplitOrientation) -> (Session, PaneId, PaneId) {
        let pid_a = PaneId::new();
        let pid_b = PaneId::new();
        let activity = Activity::terminal(ActivityId::new());
        let mut win = Session::new_with_initial(SessionId(0), "w".into(), pid_a.clone(), activity);
        win.split_pane(
            &pid_a,
            pid_b.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            orientation,
        )
        .unwrap();
        (win, pid_a, pid_b)
    }

    #[test]
    fn direction_to_axis_and_sign() {
        assert_eq!(
            direction_to_axis_sign(PaneDirection::Right),
            (SplitOrientation::Horizontal, 1)
        );
        assert_eq!(
            direction_to_axis_sign(PaneDirection::Left),
            (SplitOrientation::Horizontal, -1)
        );
        assert_eq!(
            direction_to_axis_sign(PaneDirection::Down),
            (SplitOrientation::Vertical, 1)
        );
        assert_eq!(
            direction_to_axis_sign(PaneDirection::Up),
            (SplitOrientation::Vertical, -1)
        );
    }

    #[test]
    fn find_matching_ancestor_finds_split_when_orientation_matches() {
        let (win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid_left).unwrap();
        let found = find_matching_ancestor(&win.cells, leaf, SplitOrientation::Horizontal);
        assert!(found.is_some());
    }

    #[test]
    fn find_matching_ancestor_returns_none_when_no_ancestor_matches() {
        let (win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid_left).unwrap();
        let found = find_matching_ancestor(&win.cells, leaf, SplitOrientation::Vertical);
        assert!(found.is_none());
    }

    #[test]
    fn compute_p_at_root_returns_session_axis_length() {
        let (win, _, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let root = &win.root_cell;
        let p = compute_p_at(&win.cells, root, SplitOrientation::Horizontal, 120);
        assert_eq!(p, 120);
    }

    #[test]
    fn compute_p_at_lhs_child_of_same_axis_split_is_lhs_rounded() {
        // 50/50 horizontal split of width=120 → lhs gets round(60)=60.
        let (win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid_left).unwrap();
        let p = compute_p_at(&win.cells, leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p, 60);
    }

    #[test]
    fn compute_p_at_in_cross_axis_subtree_inherits_parent_p() {
        let (win, pid_left, _) = fresh_session_with_split(SplitOrientation::Vertical);
        let leaf = win.pane_to_cell.get(&pid_left).unwrap();
        let p_horizontal = compute_p_at(&win.cells, leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_horizontal, 120);
    }

    #[test]
    fn satisfies_min_at_pane_passes_when_p_at_or_above_min() {
        let (win, pid, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid).unwrap();
        assert!(satisfies_min_at(
            &win.cells,
            leaf,
            SplitOrientation::Horizontal,
            10,
            10,
        ));
        assert!(!satisfies_min_at(
            &win.cells,
            leaf,
            SplitOrientation::Horizontal,
            9,
            10,
        ));
    }

    #[test]
    fn satisfies_min_at_same_axis_split_walks_both_children() {
        let (win, pid, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid).unwrap();
        assert!(satisfies_min_at(
            &win.cells,
            leaf,
            SplitOrientation::Horizontal,
            109,
            10,
        ));
    }

    #[test]
    fn satisfies_min_at_codex_worked_example_rhs_subtree() {
        // §7.2 worked example: rhs subtree is 10/100 weighted with min=10.
        // P=110 OK; P=105 OK (lhs=round(105*10/110)=10); P=104 NOT OK.
        let pid_a = PaneId::new();
        let pid_b = PaneId::new();
        let pid_c = PaneId::new();
        let mut win = Session::new_with_initial(
            SessionId(0),
            "w".into(),
            pid_a.clone(),
            Activity::terminal(ActivityId::new()),
        );
        win.split_pane(
            &pid_a,
            pid_b.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        win.split_pane(
            &pid_b,
            pid_c.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();

        let cell_b = win.pane_to_cell.get(&pid_b).unwrap().clone();
        let inner_split_id = win.cells.get(&cell_b).unwrap().parent().unwrap().clone();
        if let Some(Cell::Split(s)) = win.cells.get_mut(&inner_split_id) {
            s.lhs_weight = 10.0;
            s.rhs_weight = 100.0;
        }

        let inner_split = &inner_split_id;
        assert!(satisfies_min_at(
            &win.cells,
            inner_split,
            SplitOrientation::Horizontal,
            110,
            10,
        ));
        assert!(satisfies_min_at(
            &win.cells,
            inner_split,
            SplitOrientation::Horizontal,
            105,
            10,
        ));
        assert!(!satisfies_min_at(
            &win.cells,
            inner_split,
            SplitOrientation::Horizontal,
            104,
            10,
        ));
    }

    #[test]
    fn available_to_shrink_returns_zero_when_p_sub_is_zero() {
        let (win, pid, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid).unwrap();
        assert_eq!(
            available_to_shrink(&win.cells, leaf, SplitOrientation::Horizontal, 0, 10, 5),
            0
        );
    }

    #[test]
    fn available_to_shrink_caps_at_p_sub() {
        let (win, pid, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf = win.pane_to_cell.get(&pid).unwrap();
        assert_eq!(
            available_to_shrink(&win.cells, leaf, SplitOrientation::Horizontal, 15, 10, 100),
            5
        );
    }

    #[test]
    fn available_to_shrink_handles_zero_total_weight_split() {
        let (mut win, _pid_a, pid_b) = fresh_session_with_split(SplitOrientation::Horizontal);
        let cell_b = win.pane_to_cell.get(&pid_b).unwrap().clone();
        let split_id = win.cells.get(&cell_b).unwrap().parent().unwrap().clone();
        if let Some(Cell::Split(s)) = win.cells.get_mut(&split_id) {
            s.lhs_weight = 0.0;
            s.rhs_weight = 0.0;
        }
        // P=20, min=10 → 0.5 fallback gives lhs=10, rhs=10 → both meet min.
        // Shrink by 1 → P=19 → lhs=round(9.5)=10 (banker's), rhs=9 → rhs fails.
        let result = available_to_shrink(
            &win.cells,
            &split_id,
            SplitOrientation::Horizontal,
            20,
            10,
            5,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn resize_right_in_two_column_split_grows_lhs_shrinks_rhs() {
        let (mut win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        win.dimensions = Some(crate::session::session::SessionDimensions {
            cols: 120,
            rows: 40,
        });
        let outcome = resize_split_for_pane(
            &mut win.cells,
            &win.pane_to_cell,
            &pid_left,
            PaneDirection::Right,
            1,
            120,
            40,
        );
        assert_eq!(outcome, ResizePaneOutcome::Applied);
        let leaf = win.pane_to_cell.get(&pid_left).unwrap();
        let p_after = compute_p_at(&win.cells, leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_after, 61);
    }

    #[test]
    fn resize_right_in_two_column_split_with_active_in_rhs_still_shrinks_rhs() {
        let (mut win, _pid_left, pid_right) =
            fresh_session_with_split(SplitOrientation::Horizontal);
        win.dimensions = Some(crate::session::session::SessionDimensions {
            cols: 120,
            rows: 40,
        });
        let outcome = resize_split_for_pane(
            &mut win.cells,
            &win.pane_to_cell,
            &pid_right,
            PaneDirection::Right,
            1,
            120,
            40,
        );
        assert_eq!(outcome, ResizePaneOutcome::Applied);
        let leaf = win.pane_to_cell.get(&pid_right).unwrap();
        let p_after = compute_p_at(&win.cells, leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_after, 59);
    }

    #[test]
    fn resize_returns_no_op_when_no_matching_ancestor_orientation() {
        let (mut win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        win.dimensions = Some(crate::session::session::SessionDimensions {
            cols: 120,
            rows: 40,
        });
        let outcome = resize_split_for_pane(
            &mut win.cells,
            &win.pane_to_cell,
            &pid_left,
            PaneDirection::Down,
            1,
            120,
            40,
        );
        assert_eq!(outcome, ResizePaneOutcome::NoOp);
    }

    #[test]
    fn resize_clamps_at_min_cells_when_shrinking_subtree_is_at_floor() {
        let (mut win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let cell_left = win.pane_to_cell.get(&pid_left).unwrap().clone();
        let split_id = win.cells.get(&cell_left).unwrap().parent().unwrap().clone();
        if let Some(Cell::Split(s)) = win.cells.get_mut(&split_id) {
            s.lhs_weight = 110.0;
            s.rhs_weight = 10.0;
        }
        let outcome = resize_split_for_pane(
            &mut win.cells,
            &win.pane_to_cell,
            &pid_left,
            PaneDirection::Right,
            5,
            120,
            40,
        );
        assert_eq!(outcome, ResizePaneOutcome::NoOp);
    }

    #[test]
    fn resize_partially_applies_when_amount_exceeds_available_budget() {
        // 50/50 horizontal split, P=120 → 60/60 cells. min=10. Budget = 50.
        // Request 100 → should partially apply 50, leaving lhs at 110.
        let (mut win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let outcome = resize_split_for_pane(
            &mut win.cells,
            &win.pane_to_cell,
            &pid_left,
            PaneDirection::Right,
            100,
            120,
            40,
        );
        assert_eq!(outcome, ResizePaneOutcome::Applied);
        let leaf = win.pane_to_cell.get(&pid_left).unwrap();
        let p_after = compute_p_at(&win.cells, leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_after, 110);
    }

    #[test]
    fn resize_no_drift_across_repeated_one_cell_adjustments() {
        let (mut win, pid_left, _) = fresh_session_with_split(SplitOrientation::Horizontal);
        let leaf_id = win.pane_to_cell.get(&pid_left).unwrap().clone();
        let split_id = win.cells.get(&leaf_id).unwrap().parent().unwrap().clone();
        let before = if let Some(Cell::Split(s)) = win.cells.get(&split_id) {
            (s.lhs_weight, s.rhs_weight)
        } else {
            panic!("not a split")
        };
        for _ in 0..50 {
            let _ = resize_split_for_pane(
                &mut win.cells,
                &win.pane_to_cell,
                &pid_left,
                PaneDirection::Right,
                1,
                120,
                40,
            );
            let _ = resize_split_for_pane(
                &mut win.cells,
                &win.pane_to_cell,
                &pid_left,
                PaneDirection::Left,
                1,
                120,
                40,
            );
        }
        let after = if let Some(Cell::Split(s)) = win.cells.get(&split_id) {
            (s.lhs_weight, s.rhs_weight)
        } else {
            panic!("not a split")
        };
        assert!((before.0 - after.0).abs() < 1e-3);
        assert!((before.1 - after.1).abs() < 1e-3);
    }
}
