//! Close-pane shortcut action: closes the active pane when a
//! `ClosePaneActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::MultiplexerCommands;

/// Registers the `apply_close_pane` observer.
pub struct ClosePaneActionPlugin;

impl Plugin for ClosePaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_close_pane);
    }
}

/// Request to close the active pane. Triggered by `ShortcutAction::ClosePane`.
#[derive(EntityEvent, Debug)]
pub struct ClosePaneActionEvent {
    #[event_target]
    pub workspace: Entity,
}

fn apply_close_pane(trigger: On<ClosePaneActionEvent>, mut mux: MultiplexerCommands) {
    let ClosePaneActionEvent { workspace } = trigger.event();
    let Some(active_pane) = mux.workspaces_active_pane(*workspace) else {
        tracing::warn!(target: "ozmux_gui::commands", ?workspace, "ClosePane: workspace vanished");
        return;
    };
    if let Err(err) = mux.close_pane(active_pane) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "ClosePane failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{
        ActivePane, MultiplexerCommands, MultiplexerPlugin, Side, SplitOrientation,
    };

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(ClosePaneActionPlugin);
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
    fn close_pane_event_removes_pane_and_promotes_survivor() {
        let mut app = setup_app();
        let workspace = bootstrap_session(app.world_mut());
        // Split so there are 2 panes.
        let original_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let original = mux.workspaces_active_pane(workspace).unwrap();
                mux.split_pane(original, Side::After, SplitOrientation::Horizontal)
                    .unwrap();
                original
            })
            .unwrap();
        app.world_mut().flush();
        let active_before = app.world().get::<ActivePane>(workspace).map(|a| a.0).unwrap();
        assert_ne!(active_before, original_pane, "split must promote new pane");

        app.world_mut().trigger(ClosePaneActionEvent { workspace });
        app.world_mut().flush();

        let active_after = app.world().get::<ActivePane>(workspace).map(|a| a.0).unwrap();
        assert_ne!(
            active_after, active_before,
            "active pane should change after close"
        );
    }

    #[test]
    fn close_pane_event_in_single_pane_session_is_a_noop() {
        let mut app = setup_app();
        let workspace = bootstrap_session(app.world_mut());
        let pane_count_before = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| mux.panes_of_workspace(workspace).count())
            .unwrap();
        app.world_mut().trigger(ClosePaneActionEvent { workspace });
        app.world_mut().flush();
        let pane_count_after = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| mux.panes_of_workspace(workspace).count())
            .unwrap();
        assert_eq!(pane_count_after, pane_count_before);
    }

    #[test]
    fn close_pane_event_on_vanished_session_is_a_noop() {
        let mut app = setup_app();
        let bogus = app.world_mut().spawn(ozmux_multiplexer::WorkspaceMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        app.world_mut()
            .trigger(ClosePaneActionEvent { workspace: bogus });
        app.world_mut().flush();
    }
}
