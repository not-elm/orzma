//! `ZoomPaneRequest` — toggles zoom on the target tmux pane via
//! `resize-pane -Z`.

use bevy::prelude::*;
use orzma_tmux::{TmuxClient, TmuxPane, ZoomPane};

/// Toggles zoom on the tmux pane owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ZoomPaneRequest {
    /// The pane entity to zoom.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `ZoomPaneRequest` apply observer.
pub(super) struct ZoomPanePlugin;

impl Plugin for ZoomPanePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_zoom_pane);
    }
}

fn on_zoom_pane(
    ev: On<ZoomPaneRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(ZoomPane { pane: pane.id }) {
        tracing::warn!(?e, "resize-pane -Z send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{CellDims, PaneId};

    #[test]
    fn zoom_pane_sends_resize_z() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_zoom_pane);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let target = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(2),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.world_mut().trigger(ZoomPaneRequest { entity: target });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("resize-pane -Z -t %2"), "got {out:?}");
    }
}
