//! Focus-activity shortcut action: cycles the active pane's focused
//! activity when a `FocusActivityActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::{CycleDirection, MultiplexerCommands};

/// Registers the `apply_focus_activity` observer.
pub struct FocusActivityActionPlugin;

impl Plugin for FocusActivityActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_focus_activity);
    }
}

/// Request to cycle the active pane's focused activity in `direction`.
/// Triggered by `ShortcutAction::FocusActivity`.
#[derive(EntityEvent, Debug)]
pub struct FocusActivityActionEvent {
    #[event_target]
    pub session: Entity,
    pub direction: CycleDirection,
}

fn apply_focus_activity(trigger: On<FocusActivityActionEvent>, mut mux: MultiplexerCommands) {
    let FocusActivityActionEvent { session, direction } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "FocusActivity: session vanished");
        return;
    };
    let Some(active_activity) = mux.panes_active_activity(active_pane) else {
        tracing::warn!(target: "ozmux_gui::commands", ?active_pane, "FocusActivity: pane vanished");
        return;
    };

    let activities: Vec<Entity> = mux.activities_of_pane(active_pane).collect();
    if activities.len() < 2 {
        return;
    }

    let i = activities
        .iter()
        .position(|a| *a == active_activity)
        .unwrap_or(0);
    let len = activities.len() as isize;
    let delta: isize = match *direction {
        CycleDirection::Next => 1,
        CycleDirection::Prev => -1,
    };
    let j = ((i as isize + delta).rem_euclid(len)) as usize;
    let target = activities[j];

    if let Err(err) = mux.set_active_activity(active_pane, target) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusActivity failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{
        ActiveActivity, ActivePane, ActivityKind, MultiplexerCommands, MultiplexerPlugin,
    };

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(FocusActivityActionPlugin);
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
    fn focus_activity_next_advances_active_activity() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        // Add a second activity so we have something to cycle to.
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let a = mux.add_activity(active_pane, ActivityKind::Terminal);
                mux.set_active_activity(active_pane, a).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        let current_active = app
            .world()
            .get::<ActiveActivity>(active_pane)
            .map(|a| a.0)
            .unwrap();
        // Reset to first activity so the Next test advances it.
        let first_activity = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.activities_of_pane(active_pane)
                    .find(|a| *a != current_active)
            })
            .unwrap()
            .expect("second activity exists");
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_activity(active_pane, first_activity)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().trigger(FocusActivityActionEvent {
            session,
            direction: CycleDirection::Next,
        });
        app.world_mut().flush();

        let active_after = app
            .world()
            .get::<ActiveActivity>(active_pane)
            .map(|a| a.0)
            .unwrap();
        assert_ne!(active_after, first_activity);
    }

    #[test]
    fn focus_activity_in_single_activity_pane_is_a_noop() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        let active_before = app
            .world()
            .get::<ActiveActivity>(active_pane)
            .map(|a| a.0)
            .unwrap();
        app.world_mut().trigger(FocusActivityActionEvent {
            session,
            direction: CycleDirection::Next,
        });
        app.world_mut().flush();
        let active_after = app
            .world()
            .get::<ActiveActivity>(active_pane)
            .map(|a| a.0)
            .unwrap();
        assert_eq!(active_after, active_before);
    }
}
