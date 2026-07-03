//! `KillWindowRequest` — opens a confirm prompt that kills the target window
//! on `y` (mirrors tmux's default confirm-wrapped `kill-window` binding).

use crate::ui::tmux::confirm_prompt::ConfirmState;
use bevy::prelude::*;
use ozmux_tmux::{KillWindow, TmuxClient, TmuxCommand, TmuxWindow};

/// Asks for confirmation, then kills the tmux window owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct KillWindowRequest {
    /// The window entity to kill.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `KillWindowRequest` apply observer.
pub(super) struct KillWindowPlugin;

impl Plugin for KillWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_kill_window);
    }
}

fn on_kill_window(
    ev: On<KillWindowRequest>,
    mut commands: Commands,
    windows: Query<&TmuxWindow>,
    clients: Query<(), With<TmuxClient>>,
) {
    let Ok(window) = windows.get(ev.entity) else {
        return;
    };
    // NOTE: without a live client the confirmed kill would be silently
    // dropped by the prompt's send path — opening the modal would capture
    // the keyboard for a no-op, so bail before prompting.
    if clients.is_empty() {
        return;
    }
    commands.insert_resource(ConfirmState {
        message: format!("kill-window {}? (y/n)", window.name),
        command: KillWindow { window: window.id }.into_raw_command(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::WindowId;

    #[test]
    fn kill_window_opens_confirm_with_window_name() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_kill_window);
        app.world_mut().spawn(TmuxClient::new_adopted());
        let target = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(2),
                index: 1,
                name: "editor".into(),
            })
            .id();
        app.world_mut()
            .trigger(KillWindowRequest { entity: target });
        app.update();
        let state = app.world().resource::<ConfirmState>();
        assert_eq!(state.message, "kill-window editor? (y/n)");
        assert_eq!(state.command, "kill-window -t @2");
    }

    #[test]
    fn kill_window_without_client_does_not_prompt() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_kill_window);
        let target = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(1),
                index: 0,
                name: "shell".into(),
            })
            .id();
        app.world_mut()
            .trigger(KillWindowRequest { entity: target });
        app.update();
        assert!(!app.world().contains_resource::<ConfirmState>());
    }
}
