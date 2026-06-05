//! Direction-resolution algorithm for pane focus movement. Owns `PaneDirection`,
//! `CycleDirection`, and the pure adjacency / overlap helpers. No I/O.

use crate::error::{MultiplexerError, MultiplexerResult};
use crate::layout::{LayoutTree, Rect, pane_bounds};
use bevy::ecs::entity::Entity;

/// Cardinal direction for pane-focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Resolve the pane that should receive focus when moving `direction` from
/// `from` within the layout tree rooted at `root` (the layout-root node).
///
/// Returns `Ok(None)` when no candidate exists (single-pane workspace or
/// pathological layout); never picks `from` itself.
///
/// `score` assigns a tiebreaking weight to each candidate entity; the
/// candidate with the highest score wins. Pass `|_| 0` for no preference.
pub fn pane_in_direction(
    tree: &LayoutTree,
    root: Entity,
    from: Entity,
    direction: PaneDirection,
    score: impl Fn(Entity) -> u64,
) -> MultiplexerResult<Option<Entity>> {
    let panes = pane_bounds(tree, root);
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
    use crate::commands::MultiplexerCommands;
    use crate::components::WorkspaceUiSubtree;
    use crate::layout::{Side, SplitOrientation};
    use bevy::ecs::system::{In, RunSystemOnce};
    use bevy::prelude::App;
    use bevy::prelude::MinimalPlugins;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::plugin::MultiplexerPlugin);
        app
    }

    fn create_workspace(app: &mut App) -> (Entity, Entity) {
        let created = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        app.world_mut().flush();
        (created.workspace, created.pane)
    }

    fn split(app: &mut App, target: Entity, orientation: SplitOrientation) -> Entity {
        let new_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(target, Side::After, orientation).unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        new_pane
    }

    fn root_of(app: &App, workspace: Entity) -> Entity {
        app.world().get::<WorkspaceUiSubtree>(workspace).unwrap().0
    }

    fn resolve(
        app: &mut App,
        root: Entity,
        from: Entity,
        direction: PaneDirection,
    ) -> Option<Entity> {
        resolve_scored(app, root, from, direction, |_| 0)
    }

    fn resolve_scored(
        app: &mut App,
        root: Entity,
        from: Entity,
        direction: PaneDirection,
        score: impl Fn(Entity) -> u64 + Send + Sync + 'static,
    ) -> Option<Entity> {
        app.world_mut()
            .run_system_once_with(
                move |In((root, from)): In<(Entity, Entity)>, tree: LayoutTree| {
                    pane_in_direction(&tree, root, from, direction, &score).unwrap()
                },
                (root, from),
            )
            .unwrap()
    }

    fn bounds_of(app: &mut App, root: Entity) -> Vec<(Entity, Rect)> {
        app.world_mut()
            .run_system_once_with(
                |In(root): In<Entity>, tree: LayoutTree| pane_bounds(&tree, root),
                root,
            )
            .unwrap()
    }

    #[test]
    fn horizontal_split_right_then_left_wraps() {
        let mut app = new_app();
        let (ws, left) = create_workspace(&mut app);
        let right = split(&mut app, left, SplitOrientation::Horizontal);
        let root = root_of(&app, ws);

        assert_eq!(
            resolve(&mut app, root, left, PaneDirection::Right),
            Some(right),
        );
        assert_eq!(
            resolve(&mut app, root, left, PaneDirection::Left),
            Some(right),
            "wrap from left edge picks the rightmost pane",
        );
        assert_eq!(
            resolve(&mut app, root, right, PaneDirection::Up),
            None,
            "1xN strip has no candidate on the perpendicular axis",
        );
    }

    #[test]
    fn vertical_split_down_and_up_wrap() {
        let mut app = new_app();
        let (ws, top) = create_workspace(&mut app);
        let bottom = split(&mut app, top, SplitOrientation::Vertical);
        let root = root_of(&app, ws);

        assert_eq!(
            resolve(&mut app, root, top, PaneDirection::Down),
            Some(bottom),
        );
        assert_eq!(
            resolve(&mut app, root, top, PaneDirection::Up),
            Some(bottom),
            "wrap from top edge",
        );
    }

    #[test]
    fn single_pane_returns_none_in_all_directions() {
        let mut app = new_app();
        let (ws, p) = create_workspace(&mut app);
        let root = root_of(&app, ws);
        assert_eq!(
            bounds_of(&mut app, root),
            vec![(
                p,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0
                }
            )],
            "the single pane gets the full unit rect",
        );
        for d in [
            PaneDirection::Up,
            PaneDirection::Down,
            PaneDirection::Left,
            PaneDirection::Right,
        ] {
            assert_eq!(resolve(&mut app, root, p, d), None);
        }
    }

    #[test]
    fn two_by_two_grid_picks_geometric_neighbor() {
        let mut app = new_app();
        let (ws, p1) = create_workspace(&mut app);
        let p2 = split(&mut app, p1, SplitOrientation::Horizontal);
        let p3 = split(&mut app, p1, SplitOrientation::Vertical);
        let p4 = split(&mut app, p2, SplitOrientation::Vertical);
        let root = root_of(&app, ws);

        let bounds = bounds_of(&mut app, root);
        let rect = |e: Entity| bounds.iter().find(|(p, _)| *p == e).unwrap().1;
        let (tl, tr, bl, br) = (p1, p2, p3, p4);
        assert!(rect(tl).x < rect(tr).x && rect(tl).y < rect(bl).y);
        assert!(rect(br).x > rect(bl).x && rect(br).y > rect(tr).y);

        assert_eq!(resolve(&mut app, root, tl, PaneDirection::Right), Some(tr));
        assert_eq!(resolve(&mut app, root, tl, PaneDirection::Down), Some(bl));
        assert_eq!(resolve(&mut app, root, br, PaneDirection::Left), Some(bl));
        assert_eq!(resolve(&mut app, root, br, PaneDirection::Up), Some(tr));
    }

    #[test]
    fn deep_horizontal_split_keeps_immediate_neighbor() {
        let mut app = new_app();
        let (ws, first) = create_workspace(&mut app);
        let mut current_pane = first;
        let mut second_last_pane = first;
        for _ in 2..=21_u32 {
            second_last_pane = current_pane;
            current_pane = split(&mut app, current_pane, SplitOrientation::Horizontal);
        }
        let root = root_of(&app, ws);
        assert_eq!(
            resolve(&mut app, root, current_pane, PaneDirection::Left),
            Some(second_last_pane),
        );
    }

    #[test]
    fn tiebreak_prefers_most_recent_active_point() {
        let mut app = new_app();
        let (ws, tl) = create_workspace(&mut app);
        let r = split(&mut app, tl, SplitOrientation::Horizontal);
        let bl = split(&mut app, tl, SplitOrientation::Vertical);
        let root = root_of(&app, ws);

        let scores_tl_higher = move |p: Entity| {
            if p == tl {
                2u64
            } else if p == bl {
                1
            } else {
                0
            }
        };
        assert_eq!(
            resolve_scored(&mut app, root, r, PaneDirection::Left, scores_tl_higher),
            Some(tl),
            "tl has higher score so wins tiebreak",
        );

        let scores_bl_higher = move |p: Entity| {
            if p == bl {
                2u64
            } else if p == tl {
                1
            } else {
                0
            }
        };
        assert_eq!(
            resolve_scored(&mut app, root, r, PaneDirection::Left, scores_bl_higher),
            Some(bl),
        );
    }
}
