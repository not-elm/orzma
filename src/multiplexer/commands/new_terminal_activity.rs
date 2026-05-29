use bevy::prelude::*;
use ozmux_multiplexer::{ActivityKind, MultiplexerCommands};

pub struct NewTerminalActivityActionPlugin;

impl Plugin for NewTerminalActivityActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_new_terminal_activity);
    }
}

#[derive(EntityEvent, Debug)]
pub struct NewTerminalActivityEvent {
    #[event_target]
    pub session: Entity,
}

fn apply_new_terminal_activity(
    trigger: On<NewTerminalActivityEvent>,
    mut mux: MultiplexerCommands,
) {
    let NewTerminalActivityEvent { session } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "NewActivity: session vanished");
        return;
    };
    let new_activity = mux.add_activity(active_pane, ActivityKind::Terminal);
    if let Err(err) = mux.set_active_activity(active_pane, new_activity) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "NewActivity: set_active_activity failed");
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
        app.add_plugins(NewTerminalActivityActionPlugin);
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
    fn new_terminal_activity_event_adds_and_activates_activity_on_active_pane() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        app.world_mut()
            .trigger(NewTerminalActivityEvent { session });
        app.world_mut().flush();
        let activity_count = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.activities_of_pane(active_pane).count()
            })
            .unwrap();
        assert_eq!(activity_count, 2);
    }

    #[test]
    fn new_terminal_activity_event_on_vanished_session_is_a_noop() {
        let mut app = setup_app();
        let bogus = app.world_mut().spawn(ozmux_multiplexer::SessionMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        // Triggering on a despawned entity must not panic and must not mutate state.
        app.world_mut()
            .trigger(NewTerminalActivityEvent { session: bogus });
        app.world_mut().flush();
    }
}
