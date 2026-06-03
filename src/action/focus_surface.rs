//! Focus-surface shortcut action: cycles the active pane's focused
//! surface when a `FocusSurfaceActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::{CycleDirection, MultiplexerCommands};

/// Registers the `apply_focus_surface` observer.
pub struct FocusSurfaceActionPlugin;

impl Plugin for FocusSurfaceActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_focus_surface);
    }
}

/// Request to cycle the active pane's focused surface in `direction`.
/// Triggered by `ShortcutAction::FocusSurface`.
#[derive(EntityEvent, Debug)]
pub struct FocusSurfaceActionEvent {
    #[event_target]
    pub session: Entity,
    pub direction: CycleDirection,
}

fn apply_focus_surface(trigger: On<FocusSurfaceActionEvent>, mut mux: MultiplexerCommands) {
    let FocusSurfaceActionEvent { session, direction } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "FocusSurface: session vanished");
        return;
    };
    let Some(active_surface) = mux.panes_active_surface(active_pane) else {
        tracing::warn!(target: "ozmux_gui::commands", ?active_pane, "FocusSurface: pane vanished");
        return;
    };

    let surfaces: Vec<Entity> = mux.surfaces_of_pane(active_pane).collect();
    if surfaces.len() < 2 {
        return;
    }

    let i = surfaces
        .iter()
        .position(|a| *a == active_surface)
        .unwrap_or(0);
    let len = surfaces.len() as isize;
    let delta: isize = match *direction {
        CycleDirection::Next => 1,
        CycleDirection::Prev => -1,
    };
    let j = ((i as isize + delta).rem_euclid(len)) as usize;
    let target = surfaces[j];

    if let Err(err) = mux.set_active_surface(active_pane, target) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusSurface failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{
        ActivePane, ActiveSurface, MultiplexerCommands, MultiplexerPlugin, SurfaceKind,
    };

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(FocusSurfaceActionPlugin);
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
    fn focus_surface_next_advances_active_surface() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        // Add a second surface so we have something to cycle to.
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let a = mux.add_surface(active_pane, SurfaceKind::Terminal);
                mux.set_active_surface(active_pane, a).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        let current_active = app
            .world()
            .get::<ActiveSurface>(active_pane)
            .map(|a| a.0)
            .unwrap();
        // Reset to first surface so the Next test advances it.
        let first_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.surfaces_of_pane(active_pane)
                    .find(|a| *a != current_active)
            })
            .unwrap()
            .expect("second surface exists");
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(active_pane, first_surface).unwrap();
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().trigger(FocusSurfaceActionEvent {
            session,
            direction: CycleDirection::Next,
        });
        app.world_mut().flush();

        let active_after = app
            .world()
            .get::<ActiveSurface>(active_pane)
            .map(|a| a.0)
            .unwrap();
        assert_ne!(active_after, first_surface);
    }

    #[test]
    fn focus_surface_in_single_surface_pane_is_a_noop() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        let active_before = app
            .world()
            .get::<ActiveSurface>(active_pane)
            .map(|a| a.0)
            .unwrap();
        app.world_mut().trigger(FocusSurfaceActionEvent {
            session,
            direction: CycleDirection::Next,
        });
        app.world_mut().flush();
        let active_after = app
            .world()
            .get::<ActiveSurface>(active_pane)
            .map(|a| a.0)
            .unwrap();
        assert_eq!(active_after, active_before);
    }
}
