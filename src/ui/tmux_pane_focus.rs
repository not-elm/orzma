//! Pane augmentation and dim: gives each tmux pane a `Button` click target
//! with `FocusPolicy::Block` (load-bearing: stops pane clicks reaching webview
//! surfaces), and dims every inactive pane at the renderer via `PaneDim` (a
//! brightness multiplier the terminal shader applies) rather than an opaque
//! overlay veil. `select-pane` on press is now owned by the tmux mouse
//! gesture arbiter (`tmux_mouse::OzmuxTmuxMousePlugin`).

use crate::configs::OzmuxConfigsResource;
use crate::ui::workspace::inactive_dim_factor;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::material::PaneDim;
use ozmux_tmux::{ActivePane, TmuxPane, TmuxProjectionSet};

/// Registers the pane augmentation (adds `Button` + `FocusPolicy::Block`) and
/// dim systems. `select-pane` on press is handled by the gesture arbiter.
pub struct OzmuxTmuxPaneFocusPlugin;

impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_tmux_pane.after(TmuxProjectionSet),
                sync_pane_dim.run_if(pane_active_state_changed),
            ),
        );
    }
}

/// Gives each rendered pane (one that has its `TerminalHandle` but no `Button`
/// yet) a `Button` click target with an explicit `FocusPolicy::Block`. The
/// `Without<Button>` filter makes this run exactly once per pane.
///
/// `FocusPolicy::Block` is provided explicitly because `Button`'s required
/// `FocusPolicy::Block` is silently skipped when the pane already carries
/// `FocusPolicy::Pass` (from `Node`'s required-component default). An explicit
/// insert in the bundle replaces the existing component.
fn augment_tmux_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, With<TerminalHandle>, Without<Button>)>,
) {
    for pane in panes.iter() {
        commands.entity(pane).insert((Button, FocusPolicy::Block));
    }
}

/// True when a pane's active state may have changed this frame: a new pane
/// appeared, or the `ActivePane` marker was inserted/removed.
fn pane_active_state_changed(
    mut removed_active: RemovedComponents<ActivePane>,
    added_panes: Query<(), Added<TmuxPane>>,
    added_active: Query<(), Added<ActivePane>>,
) -> bool {
    added_panes.iter().next().is_some()
        || added_active.iter().next().is_some()
        || removed_active.read().next().is_some()
}

/// Sets each pane entity's [`PaneDim`] brightness: `1.0` for the pane carrying
/// `ActivePane` (or for all panes when no pane is active), the configured dim
/// factor otherwise. Only inserts when the value changes.
fn sync_pane_dim(
    mut commands: Commands,
    panes: Query<(Entity, Has<ActivePane>, Option<&PaneDim>), With<TmuxPane>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    let any_active = panes.iter().any(|(_, active, _)| active);
    for (entity, active, current) in panes.iter() {
        let want = if active || !any_active {
            1.0
        } else {
            dim_factor
        };
        if current.map(|d| d.0) != Some(want) {
            commands.entity(entity).insert(PaneDim(want));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxPane;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tmux_control_parser::{CellDims, PaneId};

    fn dims() -> CellDims {
        CellDims {
            width: 10,
            height: 5,
            xoff: 0,
            yoff: 0,
        }
    }

    #[test]
    fn sync_sets_pane_dim_from_active_marker() {
        use ozmux_tmux::ActivePane;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        app.insert_resource(OzmuxConfigsResource::default());
        let h = || TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));
        let p1 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                h(),
                ActivePane,
            ))
            .id();
        let p2 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims: dims(),
                },
                h(),
            ))
            .id();
        let dim = |app: &App, e| app.world().get::<PaneDim>(e).map(|d| d.0);

        app.update();
        assert_eq!(dim(&app, p1), Some(1.0), "active pane full-bright");
        assert_eq!(dim(&app, p2), Some(0.5), "inactive pane dimmed");

        // Move ActivePane to p2.
        app.world_mut().entity_mut(p1).remove::<ActivePane>();
        app.world_mut().entity_mut(p2).insert(ActivePane);
        app.update();
        assert_eq!(dim(&app, p1), Some(0.5));
        assert_eq!(dim(&app, p2), Some(1.0));

        // No active pane: both full-bright.
        app.world_mut().entity_mut(p2).remove::<ActivePane>();
        app.update();
        assert_eq!(dim(&app, p1), Some(1.0));
        assert_eq!(dim(&app, p2), Some(1.0));
    }

    #[test]
    fn augment_adds_button_and_focus_block_no_overlay() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false))),
            ))
            .id();
        app.update();

        assert!(
            app.world().get::<Button>(pane).is_some(),
            "pane gains a Button"
        );
        assert_eq!(
            app.world().get::<FocusPolicy>(pane).copied(),
            Some(FocusPolicy::Block),
            "pane gets FocusPolicy::Block to capture clicks",
        );
        let children = app
            .world()
            .get::<Children>(pane)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children, 0, "no overlay child spawned");

        app.update();
        let children_after = app
            .world()
            .get::<Children>(pane)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children_after, 0, "augment runs exactly once per pane");
    }
}
