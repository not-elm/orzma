//! Close-surface shortcut action: closes the active surface (or its pane
//! when it is the last one) when a `CloseSurfaceActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::MultiplexerCommands;

/// Registers the `apply_close_surface` observer.
pub struct CloseSurfaceActionPlugin;

impl Plugin for CloseSurfaceActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_close_surface);
    }
}

/// Request to close the active surface. Triggered by
/// `ShortcutAction::CloseSurface`.
#[derive(EntityEvent, Debug)]
pub struct CloseSurfaceActionEvent {
    #[event_target]
    pub workspace: Entity,
}

fn apply_close_surface(trigger: On<CloseSurfaceActionEvent>, mut mux: MultiplexerCommands) {
    let CloseSurfaceActionEvent { workspace } = trigger.event();
    let Some(active_pane) = mux.workspaces_active_pane(*workspace) else {
        tracing::warn!(target: "ozmux_gui::commands", ?workspace, "CloseSurface: workspace vanished");
        return;
    };
    let Some(active_surface) = mux.panes_active_surface(active_pane) else {
        tracing::warn!(target: "ozmux_gui::commands", ?active_pane, "CloseSurface: pane vanished");
        return;
    };

    let surface_count = mux.surfaces_of_pane(active_pane).count();
    if surface_count > 1 {
        if let Err(err) = mux.close_surface(active_pane, active_surface) {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "CloseSurface (multi): close_surface failed");
        }
        return;
    }

    if let Err(err) = mux.close_pane(active_pane) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "CloseSurface (single): close_pane failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{ActivePane, MultiplexerCommands, MultiplexerPlugin};

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(CloseSurfaceActionPlugin);
        app
    }

    fn bootstrap_workspace(world: &mut World) -> Entity {
        world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("test".into())).workspace
            })
            .unwrap()
    }

    #[test]
    fn close_surface_event_in_single_surface_pane_is_a_noop() {
        let mut app = setup_app();
        let workspace = bootstrap_workspace(app.world_mut());
        let active_before = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
        app.world_mut()
            .trigger(CloseSurfaceActionEvent { workspace });
        app.world_mut().flush();
        // Single-surface pane is closed via close_pane; only one pane exists,
        // so the close-last-pane invariant inside ozmux_multiplexer prevents
        // the pane from going away. Active pane identity stays put.
        let active_after = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
        assert_eq!(active_after, active_before);
    }
}
