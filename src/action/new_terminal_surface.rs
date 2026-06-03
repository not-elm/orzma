//! New-terminal-surface shortcut action: adds a Terminal Surface to the
//! active pane and focuses it when a `NewTerminalSurfaceActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::{MultiplexerCommands, SurfaceKind};

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
    pub session: Entity,
}

fn apply_new_terminal_surface(
    trigger: On<NewTerminalSurfaceActionEvent>,
    mut mux: MultiplexerCommands,
) {
    let NewTerminalSurfaceActionEvent { session } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "NewSurface: session vanished");
        return;
    };
    let new_surface = mux.add_surface(active_pane, SurfaceKind::Terminal);
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

    fn bootstrap_session(world: &mut World) -> Entity {
        world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("test".into())).session
            })
            .unwrap()
    }

    #[test]
    fn new_terminal_surface_event_adds_and_activates_surface_on_active_pane() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        app.world_mut()
            .trigger(NewTerminalSurfaceActionEvent { session });
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
    fn new_terminal_surface_event_on_vanished_session_is_a_noop() {
        let mut app = setup_app();
        let bogus = app.world_mut().spawn(ozmux_multiplexer::SessionMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        // Triggering on a despawned entity must not panic and must not mutate state.
        app.world_mut()
            .trigger(NewTerminalSurfaceActionEvent { session: bogus });
        app.world_mut().flush();
    }
}
