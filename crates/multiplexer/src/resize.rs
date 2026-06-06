//! `resize-pane` algorithm over the layout entity tree. Walks an ancestor
//! `Split` whose orientation matches the requested axis and re-weights it in
//! integer cells, writing the result back as the two children's `flex_grow`.

use crate::direction::PaneDirection;
use crate::layout::{LayoutTree, SplitOrientation, set_split_grows, split_ratio};
use bevy::prelude::*;

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

/// Resize the matching ancestor split of `pane` along `direction` by up to
/// `amount` cells. Reads the tree via `tree`, writes new grows via `commands`.
/// `workspace_cols` and `workspace_rows` are the current terminal dimensions
/// of the owning workspace.
pub fn resize_split_for_pane(
    commands: &mut Commands,
    tree: &LayoutTree,
    pane: Entity,
    direction: PaneDirection,
    amount: u16,
    workspace_cols: u16,
    workspace_rows: u16,
) -> ResizePaneOutcome {
    let (axis, sign) = direction_to_axis_sign(direction);
    let Some(ancestor) = find_matching_ancestor(tree, pane, axis) else {
        return ResizePaneOutcome::NoOp;
    };

    let workspace_p = match axis {
        SplitOrientation::Horizontal => workspace_cols,
        SplitOrientation::Vertical => workspace_rows,
    };
    let min_cells = match axis {
        SplitOrientation::Horizontal => MIN_PANE_COLS,
        SplitOrientation::Vertical => MIN_PANE_ROWS,
    };
    let p_ancestor = compute_p_at(tree, ancestor, axis, workspace_p);

    let Some((lhs, rhs)) = tree.split_children(ancestor) else {
        return ResizePaneOutcome::NoOp;
    };
    let (current_lhs, current_rhs) = split_cells(p_ancestor, tree.grow(lhs), tree.grow(rhs));

    let (shrink_cell, shrink_p) = if sign > 0 {
        (rhs, current_rhs)
    } else {
        (lhs, current_lhs)
    };

    let applied = available_to_shrink(tree, shrink_cell, axis, shrink_p, min_cells, amount);
    if applied == 0 {
        return ResizePaneOutcome::NoOp;
    }

    let signed_delta = sign * applied as i16;
    let new_lhs_cells: u16 =
        ((current_lhs as i32) + (signed_delta as i32)).clamp(0, p_ancestor as i32) as u16;
    let new_rhs_cells = p_ancestor - new_lhs_cells;
    set_split_grows(
        commands,
        lhs,
        rhs,
        new_lhs_cells as f32 / p_ancestor as f32,
        new_rhs_cells as f32 / p_ancestor as f32,
    );

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
    tree: &LayoutTree,
    start: Entity,
    axis: SplitOrientation,
) -> Option<Entity> {
    let mut cursor = tree.parent(start);
    while let Some(p) = cursor {
        if tree.orientation(p) == Some(axis) {
            return Some(p);
        }
        cursor = tree.parent(p);
    }
    None
}

fn split_cells(p: u16, lhs_w: f32, rhs_w: f32) -> (u16, u16) {
    let ratio = split_ratio(lhs_w, rhs_w);
    let lhs = ((p as f32 * ratio).round_ties_even() as u16).min(p);
    (lhs, p - lhs)
}

fn compute_p_at(
    tree: &LayoutTree,
    target: Entity,
    axis: SplitOrientation,
    workspace_p: u16,
) -> u16 {
    let mut path = vec![target];
    let mut cursor = tree.parent(target);
    while let Some(p) = cursor {
        path.push(p);
        cursor = tree.parent(p);
    }
    path.reverse();

    let mut p = workspace_p;
    for window in path.windows(2) {
        let (parent, child) = (window[0], window[1]);
        if tree.orientation(parent) != Some(axis) {
            continue;
        }
        let Some((lhs, rhs)) = tree.split_children(parent) else {
            continue;
        };
        let (lc, rc) = split_cells(p, tree.grow(lhs), tree.grow(rhs));
        p = if child == lhs { lc } else { rc };
    }
    p
}

fn satisfies_min_at(
    tree: &LayoutTree,
    cell: Entity,
    axis: SplitOrientation,
    p: u16,
    min_cells: u16,
) -> bool {
    if tree.is_pane(cell) {
        return p >= min_cells;
    }
    let Some((lhs, rhs)) = tree.split_children(cell) else {
        return true;
    };
    if tree.orientation(cell) == Some(axis) {
        let (lc, rc) = split_cells(p, tree.grow(lhs), tree.grow(rhs));
        satisfies_min_at(tree, lhs, axis, lc, min_cells)
            && satisfies_min_at(tree, rhs, axis, rc, min_cells)
    } else {
        satisfies_min_at(tree, lhs, axis, p, min_cells)
            && satisfies_min_at(tree, rhs, axis, p, min_cells)
    }
}

fn available_to_shrink(
    tree: &LayoutTree,
    cell: Entity,
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
        if satisfies_min_at(tree, cell, axis, p_sub - d, min_cells) {
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
    use crate::commands::MultiplexerCommands;
    use crate::components::WorkspaceUiSubtree;
    use crate::layout::Side;
    use bevy::ecs::system::RunSystemOnce;

    fn split_children_of(app: &App, workspace: Entity) -> (Entity, Entity) {
        let root = app.world().get::<WorkspaceUiSubtree>(workspace).unwrap().0;
        let split = app
            .world()
            .get::<Children>(root)
            .unwrap()
            .iter()
            .next()
            .unwrap();
        let kids: Vec<Entity> = app.world().get::<Children>(split).unwrap().iter().collect();
        (kids[0], kids[1])
    }

    fn grows_of(app: &App, lhs: Entity, rhs: Entity) -> (f32, f32) {
        (
            app.world().get::<Node>(lhs).unwrap().flex_grow,
            app.world().get::<Node>(rhs).unwrap().flex_grow,
        )
    }

    fn two_panes(orientation: SplitOrientation) -> (App, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::plugin::MultiplexerPlugin);
        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        app.world_mut().flush();
        let right = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, orientation)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        (app, outcome.workspace, outcome.pane, right)
    }

    fn set_dims(app: &mut App, workspace: Entity, cols: u16, rows: u16) {
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_workspace_dimensions(workspace, cols, rows);
            })
            .unwrap();
        app.world_mut().flush();
    }

    fn resize(
        app: &mut App,
        pane: Entity,
        direction: PaneDirection,
        amount: u16,
    ) -> ResizePaneOutcome {
        let outcome = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.resize_pane(pane, direction, amount).unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        outcome
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
    fn split_cells_rounds_ties_even_and_caps_at_p() {
        assert_eq!(split_cells(120, 1.0, 1.0), (60, 60));
        assert_eq!(split_cells(120, 0.0, 0.0), (60, 60));
        assert_eq!(split_cells(120, 110.0, 10.0), (110, 10));
    }

    #[test]
    fn split_ratio_handles_zero_total() {
        assert_eq!(split_ratio(0.0, 0.0), 0.5);
        assert_eq!(split_ratio(1.0, 3.0), 0.25);
    }

    #[test]
    fn resize_right_grows_lhs_shrinks_rhs() {
        let (mut app, ws, left, _right) = two_panes(SplitOrientation::Horizontal);
        set_dims(&mut app, ws, 120, 40);
        let (lhs, rhs) = split_children_of(&app, ws);
        assert_eq!(
            resize(&mut app, left, PaneDirection::Right, 1),
            ResizePaneOutcome::Applied
        );
        let (gl, gr) = grows_of(&app, lhs, rhs);
        assert!(gl > gr, "lhs grew: {gl} > {gr}");
    }

    #[test]
    fn resize_no_matching_ancestor_is_noop() {
        let (mut app, ws, left, _right) = two_panes(SplitOrientation::Horizontal);
        set_dims(&mut app, ws, 120, 40);
        assert_eq!(
            resize(&mut app, left, PaneDirection::Down, 1),
            ResizePaneOutcome::NoOp
        );
    }

    #[test]
    fn resize_clamps_at_min_cells_when_shrinking_subtree_is_at_floor() {
        let (mut app, ws, left, _right) = two_panes(SplitOrientation::Horizontal);
        set_dims(&mut app, ws, 120, 40);
        // Push the rhs to its 10-col floor via repeated resize_pane Right calls
        // (110 cells of growth = 100 steps of 1). Once at the floor, any further
        // Right resize has no shrink budget left in the Mux.
        for _ in 0..100 {
            resize(&mut app, left, PaneDirection::Right, 1);
        }
        assert_eq!(
            resize(&mut app, left, PaneDirection::Right, 5),
            ResizePaneOutcome::NoOp
        );
    }

    #[test]
    fn resize_partially_applies_when_amount_exceeds_available_budget() {
        let (mut app, ws, left, right) = two_panes(SplitOrientation::Horizontal);
        set_dims(&mut app, ws, 120, 40);
        // Asking for 100 cells of growth on a 120-col split caps so rhs lands at
        // its 10-col floor: lhs ends 110, rhs 10.
        assert_eq!(
            resize(&mut app, left, PaneDirection::Right, 100),
            ResizePaneOutcome::Applied
        );
        let (gl, gr) = grows_of(&app, left, right);
        let ratio = split_ratio(gl, gr);
        assert!(
            (ratio - 110.0 / 120.0).abs() < 1e-6,
            "lhs fraction ~ 110/120, got {ratio}"
        );
    }

    #[test]
    fn resize_in_2x2_grid_resolves_cross_axis_and_same_axis_ancestors() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::plugin::MultiplexerPlugin);
        let created = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        app.world_mut().flush();
        let ws = created.workspace;
        let p1 = created.pane;
        let p2 = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(p1, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        let _p3 = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(p1, Side::After, SplitOrientation::Vertical)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        let _p4 = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(p2, Side::After, SplitOrientation::Vertical)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        set_dims(&mut app, ws, 120, 40);

        // The outer SplitH is the root's single child; its two children are the
        // SplitV subtrees holding {p1,p3} and {p2,p4}. Resizing p1 Right must
        // walk past the cross-axis SplitV parent (find_matching_ancestor) and
        // inherit p through it (compute_p_at) to land on the outer SplitH.
        let (outer_lhs, outer_rhs) = split_children_of(&app, ws);
        assert_eq!(
            resize(&mut app, p1, PaneDirection::Right, 5),
            ResizePaneOutcome::Applied
        );
        let (gl, gr) = grows_of(&app, outer_lhs, outer_rhs);
        assert!(gl > gr, "outer lhs subtree grew: {gl} > {gr}");

        // Resizing p1 Down matches the INNER SplitV ancestor (same axis),
        // confirming the same-axis-match path through the nested tree.
        assert_eq!(
            resize(&mut app, p1, PaneDirection::Down, 3),
            ResizePaneOutcome::Applied
        );
    }

    #[test]
    fn resize_clamps_via_recursive_min_check_in_same_axis_chain() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::plugin::MultiplexerPlugin);
        let created = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        app.world_mut().flush();
        let ws = created.workspace;
        let p1 = created.pane;
        let p2 = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(p1, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        let p3 = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(p2, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        // Tree: root -> SplitH_outer[p1, SplitH_inner[p2, p3]]. With 120 cols and
        // MIN_PANE_COLS=10, the rhs subtree (two leaves) needs >=20 cols, so p1
        // can grow to at most 100 cols before the recursive min check stops it.
        set_dims(&mut app, ws, 120, 40);

        let mut terminated = false;
        let mut iterations = 0;
        for _ in 0..200 {
            iterations += 1;
            if resize(&mut app, p1, PaneDirection::Right, 1) == ResizePaneOutcome::NoOp {
                terminated = true;
                break;
            }
        }
        assert!(
            terminated && iterations < 200,
            "growth must clamp to NoOp within 200 iterations (recursive min check); \
             ran {iterations} iterations"
        );

        // Already at the floor: a large further grow is a NoOp, and both rhs
        // leaves survived the clamp (recursion descended into SplitH_inner and
        // honored each leaf's min).
        assert_eq!(
            resize(&mut app, p1, PaneDirection::Right, 50),
            ResizePaneOutcome::NoOp
        );
        assert!(
            app.world().get::<Node>(p2).is_some(),
            "p2 leaf still present after clamp"
        );
        assert!(
            app.world().get::<Node>(p3).is_some(),
            "p3 leaf still present after clamp"
        );
    }

    #[test]
    fn resize_no_drift_across_repeated_one_cell_adjustments() {
        let (mut app, ws, left, right) = two_panes(SplitOrientation::Horizontal);
        set_dims(&mut app, ws, 120, 40);
        // Settle to a stable +1/-1 cycle once (the first write normalizes the
        // initial (1.0, 1.0) grows into fractions), then capture the baseline
        // ratio and assert it does not drift across 50 more round-trips.
        resize(&mut app, left, PaneDirection::Right, 1);
        resize(&mut app, left, PaneDirection::Left, 1);
        let (bl, br) = grows_of(&app, left, right);
        let before = split_ratio(bl, br);
        for _ in 0..50 {
            resize(&mut app, left, PaneDirection::Right, 1);
            resize(&mut app, left, PaneDirection::Left, 1);
        }
        let (al, ar) = grows_of(&app, left, right);
        let after = split_ratio(al, ar);
        assert!(
            (before - after).abs() < 1e-3,
            "ratio drift: {before} -> {after}"
        );
    }
}
