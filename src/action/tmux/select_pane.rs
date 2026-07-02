//! `SelectPaneRequest` — focuses a neighbor of the target tmux pane via
//! `select-pane -L|-D|-U|-R`.

use bevy::prelude::*;
use ozmux_tmux::{PaneDirection, SelectPaneTowards, TmuxClient, TmuxPane};

/// Focuses the `direction` neighbor of the tmux pane owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct SelectPaneRequest {
    /// The pane whose neighbor is selected.
    #[event_target]
    pub entity: Entity,
    /// Which neighbor to select.
    pub direction: PaneDirection,
}

/// Registers the `SelectPaneRequest` apply observer.
pub(super) struct SelectPanePlugin;

impl Plugin for SelectPanePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_select_pane);
    }
}

fn on_select_pane(
    ev: On<SelectPaneRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(SelectPaneTowards {
        pane: pane.id,
        direction: ev.direction,
    }) {
        tracing::warn!(?e, "select-pane send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{CellDims, PaneId};

    #[test]
    fn select_pane_sends_directional_select() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_select_pane);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let target = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(9),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.world_mut().trigger(SelectPaneRequest {
            entity: target,
            direction: PaneDirection::Left,
        });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("select-pane -L -t %9"), "got {out:?}");
    }
}
