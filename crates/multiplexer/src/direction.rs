//! Direction-resolution algorithm for pane focus movement. Owns `PaneDirection`,
//! `CycleDirection`, and the pure adjacency / overlap helpers. No I/O.

use bevy::ecs::entity::Entity;
use crate::cells::{CellId, LayoutCellState, Rect};
use crate::error::{MultiplexerError, MultiplexerResult};

/// Cardinal direction for pane-focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    /// Move focus toward the top of the session.
    Up,
    /// Move focus toward the bottom of the session.
    Down,
    /// Move focus toward the left of the session.
    Left,
    /// Move focus toward the right of the session.
    Right,
}

impl PaneDirection {
    fn primary_edge(self, rect: Rect) -> f32 {
        match self {
            Self::Up => rect.y,
            Self::Down => rect.y + rect.h,
            Self::Left => rect.x,
            Self::Right => rect.x + rect.w,
        }
    }

    fn opposite_edge(self, rect: Rect) -> f32 {
        match self {
            Self::Up => rect.y + rect.h,
            Self::Down => rect.y,
            Self::Left => rect.x + rect.w,
            Self::Right => rect.x,
        }
    }

    fn perpendicular_range(self, rect: Rect) -> (f32, f32) {
        match self {
            Self::Up | Self::Down => (rect.x, rect.x + rect.w),
            Self::Left | Self::Right => (rect.y, rect.y + rect.h),
        }
    }

    fn wrap_edge(self) -> f32 {
        match self {
            Self::Up | Self::Left => 1.0,
            Self::Down | Self::Right => 0.0,
        }
    }
}

/// Cycle direction for operations like `swap_pane` that traverse the
/// depth-first leaf order.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CycleDirection {
    /// Move to the next pane in DFS order (wraps at end).
    Next,
    /// Move to the previous pane in DFS order (wraps at start).
    Prev,
}

const PANE_ADJACENCY_EPS: f32 = 1e-7;

fn touches_edge(other: Rect, direction: PaneDirection, edge: f32) -> bool {
    (direction.opposite_edge(other) - edge).abs() < PANE_ADJACENCY_EPS
}

fn overlaps_perpendicular(me: Rect, other: Rect, direction: PaneDirection) -> bool {
    let (a0, a1) = direction.perpendicular_range(me);
    let (b0, b1) = direction.perpendicular_range(other);
    a0 + PANE_ADJACENCY_EPS < b1 && b0 + PANE_ADJACENCY_EPS < a1
}

fn pick_best(
    panes: &[(Entity, Rect)],
    from: Entity,
    me: Rect,
    direction: PaneDirection,
    edge: f32,
    score: &impl Fn(Entity) -> u64,
) -> Option<Entity> {
    let mut best: Option<(Entity, u64)> = None;
    for &(pid, _) in panes
        .iter()
        .filter(|(pid, _)| *pid != from)
        .filter(|(_, rect)| touches_edge(*rect, direction, edge))
        .filter(|(_, rect)| overlaps_perpendicular(me, *rect, direction))
    {
        let s = score(pid);
        best = match best {
            None => Some((pid, s)),
            Some((_, bs)) if s > bs => Some((pid, s)),
            Some(prev) => Some(prev),
        };
    }
    best.map(|(p, _)| p)
}

fn find_in_direction(
    me: Rect,
    direction: PaneDirection,
    panes: &[(Entity, Rect)],
    from: Entity,
    score: impl Fn(Entity) -> u64,
) -> Option<Entity> {
    if let Some(p) = pick_best(panes, from, me, direction, direction.primary_edge(me), &score) {
        return Some(p);
    }
    pick_best(panes, from, me, direction, direction.wrap_edge(), &score)
}

/// Resolve the pane that should receive focus when moving `direction` from `from`.
///
/// Returns `Ok(None)` when no candidate exists (single-pane session or
/// pathological layout); never picks `from` itself.
///
/// `score` assigns a tiebreaking weight to each candidate entity; the
/// candidate with the highest score wins. Pass `|_| 0` for no preference.
pub fn pane_in_direction(
    state: &LayoutCellState,
    root: &CellId,
    from: Entity,
    direction: PaneDirection,
    score: impl Fn(Entity) -> u64,
) -> MultiplexerResult<Option<Entity>> {
    let panes = state.pane_bounds(root)?;
    let me = panes
        .iter()
        .find(|(pid, _)| *pid == from)
        .map(|(_, r)| *r)
        .ok_or(MultiplexerError::PaneNotFound(from))?;
    Ok(find_in_direction(me, direction, &panes, from, score))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::{LayoutCellState, Side, SplitOrientation};

    fn pane(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("nonzero entity id")
    }

    fn setup_two_panes(
        orientation: SplitOrientation,
    ) -> (LayoutCellState, CellId, Entity, Entity) {
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
    fn horizontal_split_right_then_left_wraps() {
        let (state, root, left, right) =
            setup_two_panes(SplitOrientation::Horizontal);

        assert_eq!(
            pane_in_direction(&state, &root, left, PaneDirection::Right, |_| 0).unwrap(),
            Some(right),
        );
        assert_eq!(
            pane_in_direction(&state, &root, left, PaneDirection::Left, |_| 0).unwrap(),
            Some(right),
            "wrap from left edge picks the rightmost pane",
        );
        assert_eq!(
            pane_in_direction(&state, &root, right, PaneDirection::Up, |_| 0).unwrap(),
            None,
            "1xN strip has no candidate on the perpendicular axis",
        );
    }

    #[test]
    fn vertical_split_down_and_up_wrap() {
        let (state, root, top, bottom) =
            setup_two_panes(SplitOrientation::Vertical);
        assert_eq!(
            pane_in_direction(&state, &root, top, PaneDirection::Down, |_| 0).unwrap(),
            Some(bottom),
        );
        assert_eq!(
            pane_in_direction(&state, &root, top, PaneDirection::Up, |_| 0).unwrap(),
            Some(bottom),
            "wrap from top edge",
        );
    }

    #[test]
    fn single_pane_returns_none_in_all_directions() {
        let mut state = LayoutCellState::default();
        let p = pane(1);
        let (root, _) = state.new_session_layout(p);
        for d in [
            PaneDirection::Up,
            PaneDirection::Down,
            PaneDirection::Left,
            PaneDirection::Right,
        ] {
            assert_eq!(
                pane_in_direction(&state, &root, p, d, |_| 0).unwrap(),
                None,
            );
        }
    }

    #[test]
    fn two_by_two_grid_picks_geometric_neighbor() {
        let mut state = LayoutCellState::default();
        let tl = pane(1);
        let tr = pane(2);
        let bl = pane(3);
        let br = pane(4);
        let (root, cell_tl) = state.new_session_layout(tl);
        let cell_tr = state.new_pane(tr, None);
        let cell_bl = state.new_pane(bl, None);
        let cell_br = state.new_pane(br, None);
        state
            .split_cell(cell_tl, cell_tr, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        state
            .split_cell(cell_tl, cell_bl, Side::After, SplitOrientation::Vertical)
            .unwrap();
        state
            .split_cell(cell_tr, cell_br, Side::After, SplitOrientation::Vertical)
            .unwrap();

        assert_eq!(
            pane_in_direction(&state, &root, tl, PaneDirection::Right, |_| 0).unwrap(),
            Some(tr),
        );
        assert_eq!(
            pane_in_direction(&state, &root, tl, PaneDirection::Down, |_| 0).unwrap(),
            Some(bl),
        );
        assert_eq!(
            pane_in_direction(&state, &root, br, PaneDirection::Left, |_| 0).unwrap(),
            Some(bl),
        );
        assert_eq!(
            pane_in_direction(&state, &root, br, PaneDirection::Up, |_| 0).unwrap(),
            Some(tr),
        );
    }

    #[test]
    fn deep_horizontal_split_keeps_immediate_neighbor() {
        let mut state = LayoutCellState::default();
        let first = pane(1);
        let (root, mut current_cell) = state.new_session_layout(first);
        let mut current_pane = first;
        let mut second_last_pane = first;
        for n in 2..=21_u32 {
            second_last_pane = current_pane;
            let next_pane = pane(n);
            let next_cell = state.new_pane(next_pane, None);
            state
                .split_cell(current_cell, next_cell, Side::After, SplitOrientation::Horizontal)
                .unwrap();
            current_cell = next_cell;
            current_pane = next_pane;
        }
        assert_eq!(
            pane_in_direction(&state, &root, current_pane, PaneDirection::Left, |_| 0).unwrap(),
            Some(second_last_pane),
        );
    }

    #[test]
    fn tiebreak_prefers_most_recent_active_point() {
        let mut state = LayoutCellState::default();
        let tl = pane(1);
        let r = pane(2);
        let bl = pane(3);
        let (root, cell_tl) = state.new_session_layout(tl);
        let cell_r = state.new_pane(r, None);
        let cell_bl = state.new_pane(bl, None);
        state
            .split_cell(cell_tl, cell_r, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        state
            .split_cell(cell_tl, cell_bl, Side::After, SplitOrientation::Vertical)
            .unwrap();

        let scores_tl_higher = |p: Entity| if p == tl { 2u64 } else if p == bl { 1 } else { 0 };
        assert_eq!(
            pane_in_direction(&state, &root, r, PaneDirection::Left, scores_tl_higher).unwrap(),
            Some(tl),
            "tl has higher score so wins tiebreak",
        );

        let scores_bl_higher = |p: Entity| if p == bl { 2u64 } else if p == tl { 1 } else { 0 };
        assert_eq!(
            pane_in_direction(&state, &root, r, PaneDirection::Left, scores_bl_higher).unwrap(),
            Some(bl),
        );
    }
}
