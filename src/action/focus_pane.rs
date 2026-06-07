//! Focus-pane shortcut action. Moves pane focus via the Mux's geometric
//! navigation and promotes the neighbor to `ActivePane` on the workspace.
use bevy::prelude::*;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::MultiplexerCommands;
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
    pub direction: PaneDirection,
}

fn apply_focus_pane(
    trigger: On<FocusPaneActionEvent>,
    #[cfg(not(feature = "thin-client"))] mut mux: MultiplexerCommands,
    #[cfg(feature = "thin-client")] _conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        let event = trigger.event();
        let workspace = event.workspace;
        let direction = event.direction;

        let Some(from) = mux.workspaces_active_pane(workspace) else {
            tracing::debug!(target: "ozmux_gui::commands", "FocusPane: no active pane on workspace {workspace:?}");
            return;
        };

        if let Err(e) = mux.navigate(from, direction) {
            tracing::debug!(target: "ozmux_gui::commands", "FocusPane: navigate failed: {e:?}");
        }
    }
    #[cfg(feature = "thin-client")]
    {
        // TODO(T5): send ClientMessage::FocusPane over the wire.
        let _ = &trigger;
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{
        ActivePane, MultiplexerCommands, MultiplexerPlugin, Side, SplitOrientation,
    };

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(FocusPaneActionPlugin);
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
    fn focus_pane_event_in_single_pane_workspace_is_a_noop() {
        let mut app = setup_app();
        let workspace = bootstrap_workspace(app.world_mut());
        let active_before = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
        app.world_mut().trigger(FocusPaneActionEvent {
            workspace,
            direction: PaneDirection::Right,
        });
        app.world_mut().flush();
        let active_after = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
        assert_eq!(active_after, active_before);
    }

    #[test]
    fn focus_pane_right_moves_to_right_neighbor_in_horizontal_split() {
        let mut app = setup_app();

        let (workspace, left_pane) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let created = mux.create_workspace(Some("split-test".into()));
                (created.workspace, created.pane)
            })
            .unwrap();
        app.world_mut().flush();

        let right_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(left_pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();

        // NOTE: split_pane promotes the new pane to ActivePane, so reset to left.
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(workspace, left_pane).unwrap();
            })
            .unwrap();
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<ActivePane>(workspace).map(|a| a.0),
            Some(left_pane),
            "left pane must be active before the focus event",
        );

        app.world_mut().trigger(FocusPaneActionEvent {
            workspace,
            direction: PaneDirection::Right,
        });
        app.world_mut().flush();
        app.update();

        assert_eq!(
            app.world().get::<ActivePane>(workspace).map(|a| a.0),
            Some(right_pane),
            "FocusPaneActionEvent Right must move ActivePane to the right neighbor",
        );
    }
}
