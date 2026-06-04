//! New-terminal-surface shortcut action: adds a Terminal Surface to the
//! active pane and focuses it when a `NewTerminalSurfaceActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::{Cwd, MultiplexerCommands, SurfaceKind};

/// Registers the `apply_new_terminal_surface` observer.
pub struct NewTerminalSurfaceActionPlugin;

impl Plugin for NewTerminalSurfaceActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_new_terminal_surface);
    }
}

/// Request to add a new Terminal Surface to the active pane and focus it.
/// Triggered by `ShortcutAction::NewTerminalSurface`.
#[derive(EntityEvent, Debug)]
pub struct NewTerminalSurfaceActionEvent {
    #[event_target]
    pub workspace: Entity,
}

// NOTE: `mut mux` precedes `mut commands` so the new surface spawns before
// `commands` inserts its `Cwd` (sanctioned rust.md ordering exception).
fn apply_new_terminal_surface(
    trigger: On<NewTerminalSurfaceActionEvent>,
    mut mux: MultiplexerCommands,
    mut commands: Commands,
    cwds: Query<&Cwd>,
) {
    let NewTerminalSurfaceActionEvent { workspace } = trigger.event();
    let Some(active_pane) = mux.workspaces_active_pane(*workspace) else {
        tracing::warn!(target: "ozmux_gui::commands", ?workspace, "NewSurface: workspace vanished");
        return;
    };
    let seed = mux
        .panes_active_surface(active_pane)
        .and_then(|s| cwds.get(s).ok().cloned());
    let new_surface = mux.add_surface(active_pane, SurfaceKind::Terminal);
    if let Some(cwd) = seed {
        commands.entity(new_surface).insert(cwd);
    }
    if let Err(err) = mux.set_active_surface(active_pane, new_surface) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "NewSurface: set_active_surface failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{ActivePane, MultiplexerPlugin};

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(NewTerminalSurfaceActionPlugin);
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
    fn new_terminal_surface_event_adds_and_activates_surface_on_active_pane() {
        let mut app = setup_app();
        let workspace = bootstrap_workspace(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(workspace).map(|a| a.0).unwrap();
        app.world_mut()
            .trigger(NewTerminalSurfaceActionEvent { workspace });
        app.world_mut().flush();
        let surface_count = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.surfaces_of_pane(active_pane).count()
            })
            .unwrap();
        assert_eq!(surface_count, 2);
    }

    #[test]
    fn new_surface_copies_active_surface_cwd() {
        use ozmux_multiplexer::Cwd;
        let mut app = setup_app();
        let workspace = bootstrap_workspace(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(workspace).map(|a| a.0).unwrap();
        let src = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_active_surface(active_pane).unwrap()
            })
            .unwrap();
        app.world_mut().entity_mut(src).insert(Cwd("/tmp/x".into()));
        app.world_mut()
            .trigger(NewTerminalSurfaceActionEvent { workspace });
        app.world_mut().flush();
        let new = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_active_surface(active_pane).unwrap()
            })
            .unwrap();
        assert_eq!(app.world().get::<Cwd>(new), Some(&Cwd("/tmp/x".into())));
    }

    #[test]
    fn new_terminal_surface_event_on_vanished_workspace_is_a_noop() {
        let mut app = setup_app();
        let bogus = app.world_mut().spawn(ozmux_multiplexer::WorkspaceMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        // Triggering on a despawned entity must not panic and must not mutate state.
        app.world_mut()
            .trigger(NewTerminalSurfaceActionEvent { workspace: bogus });
        app.world_mut().flush();
    }
}
