//! Pane click-to-focus + dim: gives each tmux pane a `Button` click target
//! with `FocusPolicy::Block`, sends `select-pane` on click, and dims every
//! inactive pane at the renderer via `PaneDim` (a brightness multiplier the
//! terminal shader applies) rather than an opaque overlay veil.

use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::ui::workspace::inactive_dim_factor;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::material::PaneDim;
use ozmux_tmux::{TmuxConnection, TmuxPane, TmuxProjectionSet, select_pane_command};

/// Registers pane click-to-focus and dim systems.
pub struct OzmuxTmuxPaneFocusPlugin;

impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_tmux_pane.after(TmuxProjectionSet),
                focus_pane_on_click.in_set(InputPhase::Dispatch),
                sync_pane_dim.run_if(resource_exists_and_changed::<ozmux_tmux::ProjectionModel>),
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

/// Sends `select-pane -t %<id>` when the user presses a pane. Runs in
/// `InputPhase::Dispatch`. No-ops when no live tmux client is connected.
fn focus_pane_on_click(
    panes: Query<(&Interaction, &TmuxPane), Changed<Interaction>>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    for (interaction, pane) in panes.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let cmd = select_pane_command(pane.id);
        if let Err(e) = client.handle().send(&cmd) {
            tracing::warn!(?e, pane = pane.id.0, "select-pane send failed");
        }
    }
}

/// Sets each pane entity's [`PaneDim`] brightness multiplier: `1.0` for the
/// active pane, the configured dim factor for every other pane. When
/// `ProjectionModel.active_pane` is `None`, all panes are set to `1.0` (dim
/// nothing). Gated on `ProjectionModel` change; only inserts when the value
/// actually changes to avoid redundant renderer updates.
fn sync_pane_dim(
    mut commands: Commands,
    panes: Query<(Entity, &TmuxPane, Option<&PaneDim>)>,
    model: Res<ozmux_tmux::ProjectionModel>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    for (entity, pane, current) in panes.iter() {
        let active = model.active_pane == Some(pane.id) || model.active_pane.is_none();
        let want = if active { 1.0 } else { dim_factor };
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
    fn pane_press_maps_to_select_pane() {
        use ozmux_tmux::select_pane_command;
        assert_eq!(select_pane_command(PaneId(2)), "select-pane -t %2");
    }

    #[test]
    fn sync_sets_pane_dim_from_active_pane() {
        use ozmux_tmux::ProjectionModel;

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

        app.insert_resource(ProjectionModel {
            active_pane: Some(PaneId(1)),
            ..default()
        });
        app.update();
        assert_eq!(dim(&app, p1), Some(1.0), "active pane has PaneDim 1.0");
        assert_eq!(dim(&app, p2), Some(0.5), "inactive pane has PaneDim 0.5");

        app.world_mut()
            .resource_mut::<ProjectionModel>()
            .active_pane = Some(PaneId(2));
        app.update();
        assert_eq!(dim(&app, p1), Some(0.5), "p1 now inactive");
        assert_eq!(dim(&app, p2), Some(1.0), "p2 now active");

        app.world_mut()
            .resource_mut::<ProjectionModel>()
            .active_pane = None;
        app.update();
        assert_eq!(
            dim(&app, p1),
            Some(1.0),
            "no active pane: p1 is full-bright"
        );
        assert_eq!(
            dim(&app, p2),
            Some(1.0),
            "no active pane: p2 is full-bright"
        );
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
