//! `KillPaneRequest` — opens a confirm prompt that kills the target pane on
//! `y` (mirrors tmux's default confirm-wrapped `kill-pane` binding).

use crate::error::OutputLogIfFalse;
use bevy::prelude::*;
use ozmux_tmux::{KillPane, TmuxClient, TmuxPane};

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
    mut clients: Query<&mut TmuxClient>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    for mut client in clients.iter_mut() {
        client.send(KillPane { pane: pane.id }).log_err_if_failed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{CellDims, PaneId};

    fn pane(app: &mut App, id: u32) -> Entity {
        app.world_mut()
            .spawn(TmuxPane {
                id: PaneId(id),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id()
    }

    #[test]
    fn kill_pane_sends_kill_command() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_kill_pane);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let target = pane(&mut app, 5);

        app.world_mut().trigger(KillPaneRequest { entity: target });
        app.update();

        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("kill-pane -t %5"), "got {out:?}");
    }

    #[test]
    fn kill_pane_no_panic_without_client() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_kill_pane);
        let target = pane(&mut app, 1);

        app.world_mut().trigger(KillPaneRequest { entity: target });
        app.update();
    }
}
