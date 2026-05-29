//! Pure cell-algorithm for `resize-pane`. Walks an ancestor `Split`
//! whose orientation matches the requested axis and re-weights it in
//! integer cells per spec §7.

use crate::cells::{Cell, CellId, LayoutCellState, SplitOrientation};
use crate::direction::PaneDirection;
use bevy::ecs::entity::Entity;

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

/// Resolve direction → axis/sign → matching ancestor → availability → apply.
///
/// The `pane_to_cell` index is owned by `state`; callers pass the pane
/// entity directly. `session_cols` and `session_rows` are the current
/// terminal dimensions of the owning session.
pub fn resize_split_for_pane(
    state: &mut LayoutCellState,
    pane: Entity,
    direction: PaneDirection,
    amount: u16,
    session_cols: u16,
    session_rows: u16,
) -> ResizePaneOutcome {
    let Ok(leaf_id) = state.lookup_cell_for_pane(pane) else {
        return ResizePaneOutcome::NoOp;
    };
    let (axis, sign) = direction_to_axis_sign(direction);
    let Some(ancestor_id) = find_matching_ancestor(state, &leaf_id, axis) else {
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

    let (current_lhs, current_rhs, lhs_cell, rhs_cell) = match state.cell(&ancestor_id) {
        Ok(Cell::Split(s)) => {
            let (lhs, rhs) = split_cells(p_ancestor, s.lhs_weight, s.rhs_weight);
            (lhs, rhs, s.lhs_cell, s.rhs_cell)
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

    if let Ok(Cell::Split(s)) = state.cell_mut(&ancestor_id) {
        s.lhs_weight = new_lhs_w;
        s.rhs_weight = new_rhs_w;
    }

    ResizePaneOutcome::Applied
}

fn direction_to_axis_sign(d: PaneDirection) -> (SplitOrientation, i16) {
    match d {
        PaneDirection::Right => (SplitOrientation::Horizontal, 1),
        PaneDirection::Left => (SplitOrientation::Horizontal, -1),
        PaneDirection::Down => (SplitOrientation::Vertical, 1),
        PaneDirection::Up => (SplitOrientation::Vertical, -1),
    }
}

fn find_matching_ancestor(
    state: &LayoutCellState,
    start_cell: &CellId,
    axis: SplitOrientation,
) -> Option<CellId> {
    let mut cursor = state
        .cell(start_cell)
        .ok()
        .and_then(|c| c.parent().cloned());
    while let Some(parent_id) = cursor {
        let parent = state.cell(&parent_id).ok()?;
        match parent {
            Cell::Split(s) if s.orientation == axis => return Some(parent_id),
            _ => {
                cursor = parent.parent().cloned();
            }
        }
    }
    None
}

fn split_cells(p: u16, lhs_w: f32, rhs_w: f32) -> (u16, u16) {
    let ratio = LayoutCellState::split_ratio(lhs_w, rhs_w);
    let lhs = ((p as f32 * ratio).round_ties_even() as u16).min(p);
    (lhs, p - lhs)
}

fn compute_p_at(
    state: &LayoutCellState,
    target: &CellId,
    axis: SplitOrientation,
    session_cells_on_axis: u16,
) -> u16 {
    let mut path: Vec<CellId> = Vec::new();
    let mut cursor = Some(*target);
    while let Some(c) = cursor {
        path.push(c);
        cursor = state.cell(&c).ok().and_then(|cell| cell.parent().cloned());
    }
    path.reverse();

    let mut p = session_cells_on_axis;
    for window in path.windows(2) {
        let parent_id = &window[0];
        let child_id = &window[1];
        let Some(Cell::Split(s)) = state.cell(parent_id).ok() else {
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

fn satisfies_min_at(
    state: &LayoutCellState,
    cell: &CellId,
    axis: SplitOrientation,
    p: u16,
    min_cells: u16,
) -> bool {
    let Some(node) = state.cell(cell).ok() else {
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
            satisfies_min_at(state, &s.lhs_cell, axis, p, min_cells)
                && satisfies_min_at(state, &s.rhs_cell, axis, p, min_cells)
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::{Side, SplitOrientation};

    fn pane(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("nonzero entity id")
    }

    fn setup_two_panes(orientation: SplitOrientation) -> (LayoutCellState, CellId, Entity, Entity) {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let (root, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        state
            .split_cell(cell_a, cell_b, Side::After, orientation)
            .unwrap();
        (state, root, pa, pb)
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
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        let found = find_matching_ancestor(&state, &leaf, SplitOrientation::Horizontal);
        assert!(found.is_some());
    }

    #[test]
    fn find_matching_ancestor_returns_none_when_no_ancestor_matches() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        let found = find_matching_ancestor(&state, &leaf, SplitOrientation::Vertical);
        assert!(found.is_none());
    }

    #[test]
    fn compute_p_at_root_returns_session_axis_length() {
        let (state, root, _, _) = setup_two_panes(SplitOrientation::Horizontal);
        let p = compute_p_at(&state, &root, SplitOrientation::Horizontal, 120);
        assert_eq!(p, 120);
    }

    #[test]
    fn compute_p_at_lhs_child_of_same_axis_split_is_lhs_rounded() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        let p = compute_p_at(&state, &leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p, 60);
    }

    #[test]
    fn compute_p_at_in_cross_axis_subtree_inherits_parent_p() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Vertical);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        let p_horizontal = compute_p_at(&state, &leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_horizontal, 120);
    }

    #[test]
    fn satisfies_min_at_pane_passes_when_p_at_or_above_min() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        assert!(satisfies_min_at(
            &state,
            &leaf,
            SplitOrientation::Horizontal,
            10,
            10,
        ));
        assert!(!satisfies_min_at(
            &state,
            &leaf,
            SplitOrientation::Horizontal,
            9,
            10,
        ));
    }

    #[test]
    fn satisfies_min_at_same_axis_split_walks_both_children() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        assert!(satisfies_min_at(
            &state,
            &leaf,
            SplitOrientation::Horizontal,
            109,
            10,
        ));
    }

    #[test]
    fn satisfies_min_at_codex_worked_example_rhs_subtree() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let pc = pane(3);
        let (_, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        let cell_c = state.new_pane(pc, None);
        state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        state
            .split_cell(cell_b, cell_c, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let cell_b_id = state.lookup_cell_for_pane(pb).unwrap();
        let inner_split_id = state.cell(&cell_b_id).unwrap().parent().unwrap().clone();
        if let Ok(Cell::Split(s)) = state.cell_mut(&inner_split_id) {
            s.lhs_weight = 10.0;
            s.rhs_weight = 100.0;
        }

        assert!(satisfies_min_at(
            &state,
            &inner_split_id,
            SplitOrientation::Horizontal,
            110,
            10,
        ));
        assert!(satisfies_min_at(
            &state,
            &inner_split_id,
            SplitOrientation::Horizontal,
            105,
            10,
        ));
        assert!(!satisfies_min_at(
            &state,
            &inner_split_id,
            SplitOrientation::Horizontal,
            104,
            10,
        ));
    }

    #[test]
    fn available_to_shrink_returns_zero_when_p_sub_is_zero() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        assert_eq!(
            available_to_shrink(&state, &leaf, SplitOrientation::Horizontal, 0, 10, 5),
            0
        );
    }

    #[test]
    fn available_to_shrink_caps_at_p_sub() {
        let (state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        assert_eq!(
            available_to_shrink(&state, &leaf, SplitOrientation::Horizontal, 15, 10, 100),
            5
        );
    }

    #[test]
    fn available_to_shrink_handles_zero_total_weight_split() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let (_, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let cell_b_id = state.lookup_cell_for_pane(pb).unwrap();
        let split_id = state.cell(&cell_b_id).unwrap().parent().unwrap().clone();
        if let Ok(Cell::Split(s)) = state.cell_mut(&split_id) {
            s.lhs_weight = 0.0;
            s.rhs_weight = 0.0;
        }
        let result =
            available_to_shrink(&state, &split_id, SplitOrientation::Horizontal, 20, 10, 5);
        assert_eq!(result, 0);
    }

    #[test]
    fn resize_right_in_two_column_split_grows_lhs_shrinks_rhs() {
        let (mut state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let outcome = resize_split_for_pane(&mut state, pa, PaneDirection::Right, 1, 120, 40);
        assert_eq!(outcome, ResizePaneOutcome::Applied);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        let p_after = compute_p_at(&state, &leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_after, 61);
    }

    #[test]
    fn resize_right_with_active_in_rhs_still_shrinks_rhs() {
        let (mut state, _, _, pb) = setup_two_panes(SplitOrientation::Horizontal);
        let outcome = resize_split_for_pane(&mut state, pb, PaneDirection::Right, 1, 120, 40);
        assert_eq!(outcome, ResizePaneOutcome::Applied);
        let leaf = state.lookup_cell_for_pane(pb).unwrap();
        let p_after = compute_p_at(&state, &leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_after, 59);
    }

    #[test]
    fn resize_returns_no_op_when_no_matching_ancestor_orientation() {
        let (mut state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let outcome = resize_split_for_pane(&mut state, pa, PaneDirection::Down, 1, 120, 40);
        assert_eq!(outcome, ResizePaneOutcome::NoOp);
    }

    #[test]
    fn resize_clamps_at_min_cells_when_shrinking_subtree_is_at_floor() {
        let (mut state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let cell_a = state.lookup_cell_for_pane(pa).unwrap();
        let split_id = state.cell(&cell_a).unwrap().parent().unwrap().clone();
        if let Ok(Cell::Split(s)) = state.cell_mut(&split_id) {
            s.lhs_weight = 110.0;
            s.rhs_weight = 10.0;
        }
        let outcome = resize_split_for_pane(&mut state, pa, PaneDirection::Right, 5, 120, 40);
        assert_eq!(outcome, ResizePaneOutcome::NoOp);
    }

    #[test]
    fn resize_partially_applies_when_amount_exceeds_available_budget() {
        let (mut state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let outcome = resize_split_for_pane(&mut state, pa, PaneDirection::Right, 100, 120, 40);
        assert_eq!(outcome, ResizePaneOutcome::Applied);
        let leaf = state.lookup_cell_for_pane(pa).unwrap();
        let p_after = compute_p_at(&state, &leaf, SplitOrientation::Horizontal, 120);
        assert_eq!(p_after, 110);
    }

    #[test]
    fn resize_no_drift_across_repeated_one_cell_adjustments() {
        let (mut state, _, pa, _) = setup_two_panes(SplitOrientation::Horizontal);
        let cell_a = state.lookup_cell_for_pane(pa).unwrap();
        let split_id = state.cell(&cell_a).unwrap().parent().unwrap().clone();
        let before = match state.cell(&split_id).unwrap() {
            Cell::Split(s) => (s.lhs_weight, s.rhs_weight),
            _ => panic!("not a split"),
        };
        for _ in 0..50 {
            let _ = resize_split_for_pane(&mut state, pa, PaneDirection::Right, 1, 120, 40);
            let _ = resize_split_for_pane(&mut state, pa, PaneDirection::Left, 1, 120, 40);
        }
        let after = match state.cell(&split_id).unwrap() {
            Cell::Split(s) => (s.lhs_weight, s.rhs_weight),
            _ => panic!("not a split"),
        };
        assert!((before.0 - after.0).abs() < 1e-3);
        assert!((before.1 - after.1).abs() < 1e-3);
    }
}
