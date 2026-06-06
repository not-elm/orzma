//! Direction-resolution algorithm for pane-focus movement, plus the cardinal
//! / cycle / swap-offset enums. Pure geometry over `pane_bounds`: no arena
//! mutation lives here. Ported from `crates/multiplexer/src/direction.rs`.

use crate::geometry::Rect;
use crate::id::PaneId;
use serde::{Deserialize, Serialize};

/// Cardinal direction for pane-focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneDirection {
    /// Move focus toward the top of the workspace.
    Up,
    /// Move focus toward the bottom of the workspace.
    Down,
    /// Move focus toward the left of the workspace.
    Left,
    /// Move focus toward the right of the workspace.
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

/// Cycle direction for operations that traverse the depth-first leaf order
/// (`cycle_pane`).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum CycleDirection {
    /// Move to the next pane in DFS order (wraps at end).
    Next,
    /// Move to the previous pane in DFS order (wraps at start).
    Prev,
}

/// Direction of a `swap_pane` operation in the depth-first leaf traversal of
/// the layout tree.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum SwapOffset {
    /// Swap with the previous pane (wraps around at the start).
    Prev,
    /// Swap with the next pane (wraps around at the end).
    Next,
}

const PANE_ADJACENCY_EPS: f32 = 1e-7;

/// Resolve the pane that should receive focus when moving `direction` from
/// `from`, given each pane's normalized bounds (`pane_bounds` output).
///
/// Returns `None` when no candidate exists (single-pane workspace or
/// pathological layout) or when `from` is absent from `bounds`; never picks
/// `from` itself. The `score` closure assigns a tiebreaking weight to each
/// candidate; the highest score wins. Pass `|_| 0` for no preference.
pub fn pane_in_direction(
    bounds: &[(PaneId, Rect)],
    from: PaneId,
    direction: PaneDirection,
    score: impl Fn(PaneId) -> u64,
) -> Option<PaneId> {
    let me = bounds
        .iter()
        .find(|(pid, _)| *pid == from)
        .map(|(_, r)| *r)?;
    find_in_direction(me, direction, bounds, from, score)
}

fn touches_edge(other: Rect, direction: PaneDirection, edge: f32) -> bool {
    (direction.opposite_edge(other) - edge).abs() < PANE_ADJACENCY_EPS
}

fn overlaps_perpendicular(me: Rect, other: Rect, direction: PaneDirection) -> bool {
    let (a0, a1) = direction.perpendicular_range(me);
    let (b0, b1) = direction.perpendicular_range(other);
    a0 + PANE_ADJACENCY_EPS < b1 && b0 + PANE_ADJACENCY_EPS < a1
}

fn pick_best(
    panes: &[(PaneId, Rect)],
    from: PaneId,
    me: Rect,
    direction: PaneDirection,
    edge: f32,
    score: &impl Fn(PaneId) -> u64,
) -> Option<PaneId> {
    let mut best: Option<(PaneId, u64)> = None;
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
    panes: &[(PaneId, Rect)],
    from: PaneId,
    score: impl Fn(PaneId) -> u64,
) -> Option<PaneId> {
    if let Some(p) = pick_best(
        panes,
        from,
        me,
        direction,
        direction.primary_edge(me),
        &score,
    ) {
        return Some(p);
    }
    pick_best(panes, from, me, direction, direction.wrap_edge(), &score)
}

#[cfg(test)]
mod tests {
    use super::{CycleDirection, PaneDirection, SwapOffset};

    #[test]
    fn direction_enums_serde_round_trip() {
        for d in [
            PaneDirection::Up,
            PaneDirection::Down,
            PaneDirection::Left,
            PaneDirection::Right,
        ] {
            let j = serde_json::to_string(&d).unwrap();
            assert_eq!(d, serde_json::from_str::<PaneDirection>(&j).unwrap());
        }
        assert_eq!(
            SwapOffset::Next,
            serde_json::from_str(&serde_json::to_string(&SwapOffset::Next).unwrap()).unwrap()
        );
        assert_eq!(
            CycleDirection::Prev,
            serde_json::from_str(&serde_json::to_string(&CycleDirection::Prev).unwrap()).unwrap()
        );
    }
}
