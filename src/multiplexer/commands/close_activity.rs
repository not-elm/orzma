use bevy::prelude::*;
use ozmux_multiplexer::MultiplexerCommands;

pub struct CloseActivityActionPlugin;

impl Plugin for CloseActivityActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_close_activity);
    }
}

#[derive(EntityEvent, Debug)]
pub struct CloseActivityEvent {
    #[event_target]
    pub session: Entity,
}

fn apply_close_activity(trigger: On<CloseActivityEvent>, mut mux: MultiplexerCommands) {
    let CloseActivityEvent { session } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "CloseActivity: session vanished");
        return;
    };
    let Some(active_activity) = mux.panes_active_activity(active_pane) else {
        tracing::warn!(target: "ozmux_gui::commands", ?active_pane, "CloseActivity: pane vanished");
        return;
    };

    let activity_count = mux.activities_of_pane(active_pane).count();
    if activity_count > 1 {
        // TODO: despawn a single Activity without closing the Pane. Requires
        // a `despawn_activity` method on MultiplexerCommands (or equivalent)
        // that handles ActiveActivity repointing. Deferred to Task 16.
        let _ = active_activity;
        tracing::debug!(target: "ozmux_gui::commands", "CloseActivity (multi-activity): deferred to Task 16");
        return;
    }

    if let Err(err) = mux.close_pane(active_pane) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "CloseActivity (single): close_pane failed");
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
        app.add_plugins(CloseActivityActionPlugin);
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
    fn close_activity_event_in_single_activity_pane_is_a_noop() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_before = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        app.world_mut().trigger(CloseActivityEvent { session });
        app.world_mut().flush();
        // Single-activity pane is closed via close_pane; only one pane exists,
        // so the close-last-pane invariant inside ozmux_multiplexer prevents
        // the pane from going away. Active pane identity stays put.
        let active_after = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        assert_eq!(active_after, active_before);
    }
}
