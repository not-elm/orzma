//! Focus-pane shortcut action. Directional pane focus is deferred, so the
//! observer is currently a no-op (see the TODO on `apply_focus_pane`).
use bevy::prelude::*;
use ozmux_multiplexer::PaneDirection;

/// Registers the `apply_focus_pane` observer for `FocusPaneActionEvent`.
pub struct FocusPaneActionPlugin;

impl Plugin for FocusPaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_focus_pane);
    }
}

/// Request to move pane focus in `direction`. Triggered by
/// `ShortcutAction::FocusPane`.
#[derive(EntityEvent, Debug)]
pub struct FocusPaneActionEvent {
    #[event_target]
    pub workspace: Entity,
    #[expect(
        dead_code,
        reason = "populated by the shortcut dispatcher and consumed once layout-cell reads are exposed on MultiplexerCommands"
    )]
    pub direction: PaneDirection,
}

// TODO: implement direction-based focus once MultiplexerCommands exposes
// layout-cell reads + ozmux_multiplexer::direction::pane_in_direction. Until
// then the observer is a no-op to match today's apply_focus_pane behavior.
fn apply_focus_pane(trigger: On<FocusPaneActionEvent>) {
    let _ = trigger.event();
    tracing::debug!(target: "ozmux_gui::commands", "FocusPane: deferred to follow-up task");
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{ActivePane, MultiplexerCommands, MultiplexerPlugin};

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(FocusPaneActionPlugin);
        app
    }

    fn bootstrap_session(world: &mut World) -> Entity {
        world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("test".into())).workspace
            })
            .unwrap()
    }

    #[test]
    fn focus_pane_event_in_single_pane_session_is_a_noop() {
        let mut app = setup_app();
        let workspace = bootstrap_session(app.world_mut());
        let active_before = app.world().get::<ActivePane>(workspace).map(|a| a.0).unwrap();
        app.world_mut().trigger(FocusPaneActionEvent {
            workspace,
            direction: PaneDirection::Right,
        });
        app.world_mut().flush();
        let active_after = app.world().get::<ActivePane>(workspace).map(|a| a.0).unwrap();
        assert_eq!(active_after, active_before);
    }
}
