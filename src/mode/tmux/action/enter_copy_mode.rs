//! `EnterCopyModeRequest` — puts the target tmux pane into copy mode and
//! marks it with `CopyModeState` (the marker the copy-mode refresh loop and
//! wheel routing key off).

use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use ozmux_tmux::{EnterCopyMode, TmuxClient, TmuxPane};

/// Enters tmux copy mode on the pane owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct EnterCopyModeRequest {
    /// The pane entity to put into copy mode.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `EnterCopyModeRequest` apply observer.
pub(super) struct EnterCopyModePlugin;

impl Plugin for EnterCopyModePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_enter_copy_mode);
    }
}

fn on_enter_copy_mode(
    ev: On<EnterCopyModeRequest>,
    mut commands: Commands,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    match client.send(EnterCopyMode { pane: pane.id }) {
        Ok(_) => {
            commands.entity(ev.entity).insert(CopyModeState);
        }
        Err(e) => tracing::warn!(?e, "copy-mode send failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{CellDims, PaneId};

    #[test]
    fn enter_copy_mode_sends_and_marks() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_enter_copy_mode);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let target = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(8),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.world_mut()
            .trigger(EnterCopyModeRequest { entity: target });
        app.update();
        let out = {
            let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
            String::from_utf8(client.take_outgoing()).unwrap()
        };
        assert!(out.contains("copy-mode -t %8"), "got {out:?}");
        assert!(app.world().entity(target).contains::<CopyModeState>());
    }

    #[test]
    fn enter_copy_mode_without_client_does_not_mark() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_enter_copy_mode);
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
        app.world_mut()
            .trigger(EnterCopyModeRequest { entity: target });
        app.update();
        assert!(!app.world().entity(target).contains::<CopyModeState>());
    }
}
