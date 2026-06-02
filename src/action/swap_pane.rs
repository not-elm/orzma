//! Swap-pane shortcut action: swaps the active pane with a sibling when a
//! `SwapPaneActionEvent` fires.
use bevy::prelude::*;
use ozmux_multiplexer::{MultiplexerCommands, SwapOffset};

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
    pub session: Entity,
    pub offset: SwapOffset,
}

fn apply_swap_pane(trigger: On<SwapPaneActionEvent>, mut mux: MultiplexerCommands) {
    let SwapPaneActionEvent { session, offset } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "SwapPane: session vanished");
        return;
    };
    if let Err(err) = mux.swap_pane(active_pane, *offset) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "swap_pane failed");
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
        app.add_plugins(SwapPaneActionPlugin);
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
    fn swap_pane_event_in_single_pane_session_is_a_noop() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_before = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        app.world_mut().trigger(SwapPaneActionEvent {
            session,
            offset: SwapOffset::Prev,
        });
        app.world_mut().flush();
        let active_after = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        assert_eq!(active_after, active_before);
    }
}
