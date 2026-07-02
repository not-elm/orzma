//! `SplitPaneRequest` — splits the target tmux pane via `split-window`.

use bevy::prelude::*;
use ozmux_tmux::{SplitDirection, SplitWindow, TmuxClient, TmuxPane};

/// Splits the tmux pane owning `entity` in `direction`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct SplitPaneRequest {
    /// The pane entity to split.
    #[event_target]
    pub entity: Entity,
    /// tmux split direction (`-h` / `-v`).
    pub direction: SplitDirection,
}

/// Registers the `SplitPaneRequest` apply observer.
pub(super) struct SplitPanePlugin;

impl Plugin for SplitPanePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_split_pane);
    }
}

fn on_split_pane(
    ev: On<SplitPaneRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(SplitWindow {
        pane: pane.id,
        direction: ev.direction,
    }) {
        tracing::warn!(?e, "split-window send failed");
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
    fn split_pane_sends_split_window() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_split_pane);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let target = pane(&mut app, 3);
        app.world_mut().trigger(SplitPaneRequest {
            entity: target,
            direction: SplitDirection::Horizontal,
        });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("split-window -h -t %3"), "got {out:?}");
    }

    #[test]
    fn split_pane_no_panic_without_client() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_split_pane);
        let target = pane(&mut app, 1);
        app.world_mut().trigger(SplitPaneRequest {
            entity: target,
            direction: SplitDirection::Vertical,
        });
        app.update();
    }
}
