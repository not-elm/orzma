//! `KillPaneRequest` — opens a confirm prompt that kills the target pane on
//! `y` (mirrors tmux's default confirm-wrapped `kill-pane` binding).

use crate::ui::tmux::confirm_prompt::ConfirmState;
use bevy::prelude::*;
use ozmux_tmux::{KillPane, TmuxClient, TmuxCommand, TmuxPane};

/// Asks for confirmation, then kills the tmux pane owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct KillPaneRequest {
    /// The pane entity to kill.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `KillPaneRequest` apply observer.
pub(super) struct KillPanePlugin;

impl Plugin for KillPanePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_kill_pane);
    }
}

fn on_kill_pane(
    ev: On<KillPaneRequest>,
    mut commands: Commands,
    panes: Query<&TmuxPane>,
    clients: Query<(), With<TmuxClient>>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    // NOTE: without a live client the confirmed kill would be silently
    // dropped by the prompt's send path — opening the modal would capture
    // the keyboard for a no-op, so bail before prompting.
    if clients.is_empty() {
        return;
    }
    commands.insert_resource(ConfirmState {
        message: format!("kill-pane %{}? (y/n)", pane.id.0),
        command: KillPane { pane: pane.id }.into_raw_command(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{CellDims, PaneId};

    #[test]
    fn kill_pane_opens_confirm_with_kill_command() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_kill_pane);
        app.world_mut().spawn(TmuxClient::new_adopted());
        let target = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(5),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.world_mut().trigger(KillPaneRequest { entity: target });
        app.update();
        let state = app.world().resource::<ConfirmState>();
        assert_eq!(state.message, "kill-pane %5? (y/n)");
        assert_eq!(state.command, "kill-pane -t %5");
    }

    #[test]
    fn kill_pane_without_client_does_not_prompt() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_kill_pane);
        let target = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.world_mut().trigger(KillPaneRequest { entity: target });
        app.update();
        assert!(!app.world().contains_resource::<ConfirmState>());
    }
}
