//! Pane click-to-focus + dim: augments each tmux pane node with a `Button`
//! (click target) and a `FocusPolicy::Pass` dim overlay, sends `select-pane`
//! on click, and shows the overlay on every pane except the active one.

use crate::input::InputPhase;
use crate::theme;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozmux_tmux::{TmuxConnection, TmuxPane, TmuxProjectionSet, select_pane_command};

/// Points a pane at its dim-overlay child entity (O(1) lookup in `sync_pane_dim`).
#[derive(Component)]
pub(crate) struct PaneDimOverlay(pub(crate) Entity);

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
/// yet) a `Button` click target and a hidden `FocusPolicy::Pass` dim overlay
/// child, recorded on the pane as `PaneDimOverlay`. The `Without<Button>` filter makes
/// this run exactly once per pane.
fn augment_tmux_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, With<TerminalHandle>, Without<Button>)>,
) {
    for pane in panes.iter() {
        let overlay = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    ..default()
                },
                BackgroundColor(theme::PANE_DIM_OVERLAY),
                FocusPolicy::Pass,
                Visibility::Hidden,
                ChildOf(pane),
            ))
            .id();
        commands
            .entity(pane)
            .insert((Button, PaneDimOverlay(overlay)));
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

/// Shows each pane's dim overlay when the pane is not the active one, hides it
/// on the active pane. When `ProjectionModel.active_pane` is `None`, hides ALL
/// overlays (dim nothing). Gated on `ProjectionModel` change.
fn sync_pane_dim(
    mut overlays: Query<&mut Visibility>,
    panes: Query<(&TmuxPane, &PaneDimOverlay)>,
    model: Res<ozmux_tmux::ProjectionModel>,
) {
    for (pane, dim) in panes.iter() {
        let active = model.active_pane == Some(pane.id) || model.active_pane.is_none();
        let want = if active {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        // NOTE: the overlay may not be spawned yet on the frame a pane appears;
        // a `get_mut` miss is a no-op, never a panic.
        if let Ok(mut vis) = overlays.get_mut(dim.0)
            && *vis != want
        {
            *vis = want;
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
    fn sync_dims_inactive_and_clears_when_none() {
        use ozmux_tmux::ProjectionModel;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
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
        app.update(); // augment both panes (spawns overlays)

        let overlay = |app: &App, pane| app.world().get::<PaneDimOverlay>(pane).unwrap().0;
        let vis = |app: &App, e| app.world().get::<Visibility>(e).copied().unwrap();

        app.insert_resource(ProjectionModel {
            active_pane: Some(PaneId(1)),
            ..default()
        });
        app.update();
        assert_eq!(vis(&app, overlay(&app, p1)), Visibility::Hidden);
        assert_eq!(vis(&app, overlay(&app, p2)), Visibility::Visible);

        app.world_mut()
            .resource_mut::<ProjectionModel>()
            .active_pane = Some(PaneId(2));
        app.update();
        assert_eq!(vis(&app, overlay(&app, p1)), Visibility::Visible);
        assert_eq!(vis(&app, overlay(&app, p2)), Visibility::Hidden);

        app.world_mut()
            .resource_mut::<ProjectionModel>()
            .active_pane = None;
        app.update();
        assert_eq!(vis(&app, overlay(&app, p1)), Visibility::Hidden);
        assert_eq!(vis(&app, overlay(&app, p2)), Visibility::Hidden);
    }

    #[test]
    fn augment_adds_button_and_hidden_overlay() {
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
        let pane_dim = app
            .world()
            .get::<PaneDimOverlay>(pane)
            .expect("PaneDimOverlay recorded");
        let overlay = pane_dim.0;
        assert_eq!(
            app.world().get::<Visibility>(overlay).copied(),
            Some(Visibility::Hidden),
            "overlay starts hidden",
        );
        assert_eq!(
            app.world().get::<FocusPolicy>(overlay).copied(),
            Some(FocusPolicy::Pass),
            "overlay passes clicks through to the pane",
        );

        app.update();
        let children = app
            .world()
            .get::<Children>(pane)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children, 1, "augment runs exactly once per pane");
    }
}
