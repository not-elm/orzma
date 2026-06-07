//! Swap-pane shortcut action: swaps the active pane with a sibling when a
//! `SwapPaneActionEvent` fires.
use bevy::prelude::*;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::MultiplexerCommands;
use ozmux_multiplexer::SwapOffset;

/// Registers the `apply_swap_pane` observer.
pub struct SwapPaneActionPlugin;

impl Plugin for SwapPaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_swap_pane);
    }
}

/// Request to swap the active pane with the sibling at `offset`. Triggered
/// by `ShortcutAction::SwapPane`.
#[derive(EntityEvent, Debug)]
pub struct SwapPaneActionEvent {
    #[event_target]
    pub workspace: Entity,
    pub offset: SwapOffset,
}

fn apply_swap_pane(
    trigger: On<SwapPaneActionEvent>,
    #[cfg(not(feature = "thin-client"))] mut mux: MultiplexerCommands,
    #[cfg(feature = "thin-client")] _conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        let SwapPaneActionEvent { workspace, offset } = trigger.event();
        let Some(active_pane) = mux.workspaces_active_pane(*workspace) else {
            tracing::warn!(target: "ozmux_gui::commands", ?workspace, "SwapPane: workspace vanished");
            return;
        };
        if let Err(err) = mux.swap_pane(active_pane, *offset) {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "swap_pane failed");
        }
    }
    #[cfg(feature = "thin-client")]
    {
        // TODO(T5): send ClientMessage::SwapPane over the wire.
        let _ = &trigger;
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{ActivePane, MultiplexerCommands, MultiplexerPlugin};

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(SwapPaneActionPlugin);
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
    fn swap_pane_event_in_single_pane_workspace_is_a_noop() {
        let mut app = setup_app();
        let workspace = bootstrap_workspace(app.world_mut());
        let active_before = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
        app.world_mut().trigger(SwapPaneActionEvent {
            workspace,
            offset: SwapOffset::Prev,
        });
        app.world_mut().flush();
        let active_after = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
        assert_eq!(active_after, active_before);
    }
}
