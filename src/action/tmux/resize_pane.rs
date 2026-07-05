//! `ResizePaneRequest` — resizes the active tmux pane's border via
//! `resize-pane -L|-R|-U|-D -t %<id> <amount>`.

use bevy::prelude::*;
use orzma_tmux::{PaneDirection, ResizePaneTowards, TmuxClient, TmuxPane};

/// Resizes the tmux pane owning `entity` toward `direction` by a fixed step.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ResizePaneRequest {
    /// The pane to resize.
    #[event_target]
    pub entity: Entity,
    /// Which border to move.
    pub direction: PaneDirection,
}

/// Registers the `ResizePaneRequest` apply observer.
pub(super) struct ResizePanePlugin;

impl Plugin for ResizePanePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_resize_pane);
    }
}

/// Cells to move the border per resize keypress.
const RESIZE_STEP_CELLS: u32 = 5;

fn on_resize_pane(
    ev: On<ResizePaneRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(ResizePaneTowards {
        pane: pane.id,
        direction: ev.direction,
        amount: RESIZE_STEP_CELLS,
    }) {
        tracing::warn!(?e, "resize-pane send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{CellDims, PaneId};

    #[test]
    fn resize_pane_sends_directional_resize() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_resize_pane);
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
        app.world_mut().trigger(ResizePaneRequest {
            entity: target,
            direction: PaneDirection::Left,
        });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("resize-pane -L -t %9 5"), "got {out:?}");
    }
}
