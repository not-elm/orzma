//! Direction-resolution algorithm for `Window::pane_in_direction`. Owns the
//! `PaneDirection` enum and pure adjacency / overlap helpers. No I/O.

use crate::error::{MultiplexerError, MultiplexerResult};
use crate::window::cells::Rect;
use crate::window::pane::PaneId;
use crate::window::window::Window;
use serde::{Deserialize, Serialize};

/// Cardinal direction for pane-focus movement. Distinct from
/// `ozmux_configs::Direction` (UX layer) to keep the multiplexer crate free
/// of a `configs` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneDirection {
    /// Move focus toward the top of the window.
    Up,
    /// Move focus toward the bottom of the window.
    Down,
    /// Move focus toward the left of the window.
    Left,
    /// Move focus toward the right of the window.
    Right,
}

impl PaneDirection {
    /// Edge of `rect` that leads in this direction (the edge you'd cross when
    /// moving toward `self`).
    fn primary_edge(self, rect: Rect) -> f32 {
        match self {
            Self::Up => rect.y,
            Self::Down => rect.y + rect.h,
            Self::Left => rect.x,
            Self::Right => rect.x + rect.w,
        }
    }

    /// Edge of `rect` that faces away from this direction — the edge a
    /// candidate neighbor must align with to count as adjacent.
    fn opposite_edge(self, rect: Rect) -> f32 {
        match self {
            Self::Up => rect.y + rect.h,
            Self::Down => rect.y,
            Self::Left => rect.x + rect.w,
            Self::Right => rect.x,
        }
    }

    /// Perpendicular-axis range `(start, end)` of `rect` for overlap checks.
    fn perpendicular_range(self, rect: Rect) -> (f32, f32) {
        match self {
            Self::Up | Self::Down => (rect.x, rect.x + rect.w),
            Self::Left | Self::Right => (rect.y, rect.y + rect.h),
        }
    }

    /// Window-side to fold the search edge to when the primary pass finds
    /// nothing (wrap-around).
    fn wrap_edge(self) -> f32 {
        match self {
            Self::Up | Self::Left => 1.0,
            Self::Down | Self::Right => 0.0,
        }
    }
}

const PANE_ADJACENCY_EPS: f32 = 1e-7;

/// Returns true when `other`'s opposite-facing edge sits within
/// `PANE_ADJACENCY_EPS` of `edge`.
fn touches_edge(other: Rect, direction: PaneDirection, edge: f32) -> bool {
    (direction.opposite_edge(other) - edge).abs() < PANE_ADJACENCY_EPS
}

/// Half-open interval overlap of `me` and `other` along the axis perpendicular
/// to `direction`.
fn overlaps_perpendicular(me: Rect, other: Rect, direction: PaneDirection) -> bool {
    let (a0, a1) = direction.perpendicular_range(me);
    let (b0, b1) = direction.perpendicular_range(other);
    a0 + PANE_ADJACENCY_EPS < b1 && b0 + PANE_ADJACENCY_EPS < a1
}

/// Pick the candidate with the largest `score`.
fn pick_best<F: Fn(&PaneId) -> u64>(
    panes: &[(PaneId, Rect)],
    from: &PaneId,
    me: Rect,
    direction: PaneDirection,
    edge: f32,
    score: &F,
) -> Option<PaneId> {
    let mut best: Option<(&PaneId, u64)> = None;
    for (pid, _) in panes
        .iter()
        .filter(|(pid, _)| pid != from)
        .filter(|(_, rect)| touches_edge(*rect, direction, edge))
        .filter(|(_, rect)| overlaps_perpendicular(me, *rect, direction))
    {
        let score = score(pid);
        best = match best {
            None => Some((pid, score)),
            Some((_, bs)) if score > bs => Some((pid, score)),
            Some(prev) => Some(prev),
        };
    }
    best.map(|(p, _)| p.clone())
}

/// Two-pass adjacency search: primary edge first, then the opposite-side
/// wrap edge if the primary pass returned nothing.
fn find_in_direction<F: Fn(&PaneId) -> u64>(
    me: Rect,
    direction: PaneDirection,
    panes: &[(PaneId, Rect)],
    from: &PaneId,
    score: F,
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

impl Window {
    /// Resolve the pane that should receive focus when moving `direction`
    /// from `from`. Returns `Ok(None)` when no candidate exists (single-pane
    /// window or pathological layout); never picks `from` itself.
    pub fn pane_in_direction(
        &self,
        from: &PaneId,
        direction: PaneDirection,
    ) -> MultiplexerResult<Option<PaneId>> {
        let panes = self.cells.pane_bounds(&self.root_cell)?;
        let me = panes
            .iter()
            .find(|(pid, _)| pid == from)
            .map(|(_, r)| *r)
            .ok_or_else(|| MultiplexerError::PaneNotFound(from.clone()))?;
        let score = |pid: &PaneId| self.pane_active_points.get(pid).copied().unwrap_or(0);
        Ok(find_in_direction(me, direction, &panes, from, score))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::cells::{Side, SplitOrientation};
    use crate::window::pane::PaneId;
    use crate::window::pane::activity::{Activity, ActivityId};
    use crate::window::window::Window;
    use crate::window::window::WindowId;

    #[test]
    fn pane_direction_serializes_kebab_case() {
        let json = serde_json::to_string(&PaneDirection::Up).unwrap();
        assert_eq!(json, "\"up\"");
        let back: PaneDirection = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PaneDirection::Up);
    }

    fn fresh_window() -> Window {
        Window::new_with_initial(
            WindowId::new(),
            "t".into(),
            PaneId::new(),
            Activity::terminal(ActivityId::new()),
        )
    }

    fn split(window: &mut Window, target: &PaneId, orient: SplitOrientation, side: Side) -> PaneId {
        let new = PaneId::new();
        window
            .split_pane(
                target,
                new.clone(),
                Activity::terminal(ActivityId::new()),
                side,
                orient,
            )
            .unwrap();
        new
    }

    #[test]
    fn pane_in_direction_horizontal_split_right_then_left_wraps() {
        let mut w = fresh_window();
        let left = w.active_pane.clone();
        let right = split(&mut w, &left, SplitOrientation::Horizontal, Side::After);

        assert_eq!(
            w.pane_in_direction(&left, PaneDirection::Right).unwrap(),
            Some(right.clone()),
        );
        assert_eq!(
            w.pane_in_direction(&left, PaneDirection::Left).unwrap(),
            Some(right.clone()),
            "wrap from left edge picks the rightmost pane",
        );
        assert_eq!(
            w.pane_in_direction(&right, PaneDirection::Up).unwrap(),
            None,
            "1xN strip has no candidate on the perpendicular axis",
        );
    }

    #[test]
    fn pane_in_direction_vertical_split_down_and_up() {
        let mut w = fresh_window();
        let top = w.active_pane.clone();
        let bottom = split(&mut w, &top, SplitOrientation::Vertical, Side::After);
        assert_eq!(
            w.pane_in_direction(&top, PaneDirection::Down).unwrap(),
            Some(bottom.clone()),
        );
        assert_eq!(
            w.pane_in_direction(&top, PaneDirection::Up).unwrap(),
            Some(bottom.clone()),
            "wrap from top edge",
        );
    }

    #[test]
    fn pane_in_direction_single_pane_returns_none() {
        let w = fresh_window();
        for d in [
            PaneDirection::Up,
            PaneDirection::Down,
            PaneDirection::Left,
            PaneDirection::Right,
        ] {
            assert_eq!(w.pane_in_direction(&w.active_pane, d).unwrap(), None);
        }
    }

    #[test]
    fn pane_in_direction_two_by_two_grid_picks_geometric_neighbor() {
        let mut w = fresh_window();
        // Build a 2x2 grid:
        //   tl | tr
        //   ---+---
        //   bl | br
        let tl = w.active_pane.clone();
        let tr = split(&mut w, &tl, SplitOrientation::Horizontal, Side::After);
        let bl = split(&mut w, &tl, SplitOrientation::Vertical, Side::After);
        let br = split(&mut w, &tr, SplitOrientation::Vertical, Side::After);

        assert_eq!(
            w.pane_in_direction(&tl, PaneDirection::Right).unwrap(),
            Some(tr.clone())
        );
        assert_eq!(
            w.pane_in_direction(&tl, PaneDirection::Down).unwrap(),
            Some(bl.clone())
        );
        assert_eq!(
            w.pane_in_direction(&br, PaneDirection::Left).unwrap(),
            Some(bl.clone())
        );
        assert_eq!(
            w.pane_in_direction(&br, PaneDirection::Up).unwrap(),
            Some(tr.clone())
        );
    }

    #[test]
    fn pane_in_direction_deep_horizontal_split_keeps_immediate_neighbor() {
        // Repeatedly split the rightmost pane horizontally. After enough
        // levels the rightmost pane's width is below the old EPS = 1e-5,
        // which used to misfire. The new algorithm must still return its
        // immediate left neighbor (not a wrap target).
        let mut w = fresh_window();
        let mut current = w.active_pane.clone();
        let mut second_last = current.clone();
        for _ in 0..20 {
            second_last = current.clone();
            current = split(&mut w, &current, SplitOrientation::Horizontal, Side::After);
        }
        assert_eq!(
            w.pane_in_direction(&current, PaneDirection::Left).unwrap(),
            Some(second_last),
        );
    }

    #[test]
    fn pane_in_direction_tiebreak_prefers_most_recent_active_point() {
        // Layout:
        //   ┌────┬────┐
        //   │ tl │    │
        //   ├────┤ r  │
        //   │ bl │    │
        //   └────┴────┘
        // Then move Left from `r`: candidates are `tl` and `bl`. The one
        // most recently activated wins.
        let mut w = fresh_window();
        let tl = w.active_pane.clone();
        let r = split(&mut w, &tl, SplitOrientation::Horizontal, Side::After);
        let bl = split(&mut w, &tl, SplitOrientation::Vertical, Side::After);

        // Activate tl most recently.
        let _ = w.set_active_pane(&bl).unwrap();
        let _ = w.set_active_pane(&tl).unwrap();
        let _ = w.set_active_pane(&r).unwrap();
        assert_eq!(
            w.pane_in_direction(&r, PaneDirection::Left).unwrap(),
            Some(tl.clone()),
            "tl was activated more recently than bl",
        );

        // Now flip: activate bl most recently.
        let _ = w.set_active_pane(&bl).unwrap();
        let _ = w.set_active_pane(&r).unwrap();
        assert_eq!(
            w.pane_in_direction(&r, PaneDirection::Left).unwrap(),
            Some(bl.clone()),
        );
    }
}
