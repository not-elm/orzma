//! Resize outcome type and integration tests for the resize-pane operation.

/// Outcome of a resize call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizePaneOutcome {
    /// At least one cell of movement was applied; broadcast.
    Applied,
    /// No matching ancestor, or shrinking subtree has zero budget.
    NoOp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::MultiplexerCommands;
    use crate::components::{PaneDimensions, WorkspaceUiSubtree};
    use crate::direction::PaneDirection;
    use crate::layout::{Side, SplitOrientation};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

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

    fn cols_of(app: &App, pane: Entity) -> u16 {
        app.world()
            .get::<PaneDimensions>(pane)
            .map(|d| d.cols)
            .unwrap_or(0)
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
    fn resize_right_grows_lhs_shrinks_rhs() {
        let (mut app, ws, left, right) = two_panes(SplitOrientation::Horizontal);
        set_dims(&mut app, ws, 120, 40);
        let cols_before = cols_of(&app, left);
        assert_eq!(
            resize(&mut app, left, PaneDirection::Right, 1),
            ResizePaneOutcome::Applied
        );
        let lc = cols_of(&app, left);
        let rc = cols_of(&app, right);
        assert!(
            lc > cols_before,
            "lhs cols grew after resize Right: before={cols_before} after={lc}"
        );
        assert!(
            lc > rc,
            "lhs wider than rhs after resize Right: {lc} > {rc}"
        );
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
        assert_eq!(
            resize(&mut app, left, PaneDirection::Right, 100),
            ResizePaneOutcome::Applied
        );
        let lc = cols_of(&app, left);
        let rc = cols_of(&app, right);
        // Total is 120 cols; clamped at min=10, so lhs ~ 110, rhs ~ 10.
        assert_eq!(
            lc + rc,
            120,
            "cols must sum to workspace width after resize: lhs={lc} rhs={rc}"
        );
        assert!(
            lc > rc,
            "lhs must be majority after large resize Right: lhs={lc} rhs={rc}"
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

        let (outer_lhs, _outer_rhs) = split_children_of(&app, ws);
        let lhs_cols_before = cols_of(&app, p1);
        assert_eq!(
            resize(&mut app, p1, PaneDirection::Right, 5),
            ResizePaneOutcome::Applied
        );
        let lhs_cols_after = cols_of(&app, p1);
        assert!(
            lhs_cols_after > lhs_cols_before,
            "outer lhs subtree grew (p1 cols): {lhs_cols_before} -> {lhs_cols_after}"
        );
        let _ = outer_lhs;

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
        resize(&mut app, left, PaneDirection::Right, 1);
        resize(&mut app, left, PaneDirection::Left, 1);
        let before_left = cols_of(&app, left);
        for _ in 0..50 {
            resize(&mut app, left, PaneDirection::Right, 1);
            resize(&mut app, left, PaneDirection::Left, 1);
        }
        let after_left = cols_of(&app, left);
        let after_right = cols_of(&app, right);
        assert_eq!(
            before_left, after_left,
            "no drift: left cols must be stable after 50 right+left pairs: before={before_left} after={after_left}"
        );
        assert_eq!(
            after_left + after_right,
            120,
            "cols must still sum to workspace width after repeated resize: {after_left}+{after_right}"
        );
    }
}
