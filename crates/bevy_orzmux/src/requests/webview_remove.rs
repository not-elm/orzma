//! `RequestTtyWebviewRemove`: the placements the control plane asks a
//! terminal entity to drop when a registration is released, sent as
//! `OrzmuxCommand::RemovePlacements`.

use crate::requests::PaneSender;
use bevy::prelude::*;
use orzma_vt::prelude::InstanceId;
use orzmux::prelude::OrzmuxCommand;

/// Fired by the control plane to drop placements a terminal still holds
/// for registrations that are gone.
///
/// The backend cannot know that a registration was released — that
/// fact lives on the control socket — so without this the placements
/// keep a cap slot until their anchor scrolls out of history.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyWebviewRemove {
    #[event_target]
    pub terminal: Entity,
    /// The instances to drop; ids the terminal does not hold are ignored.
    pub instances: Vec<InstanceId>,
}

pub(super) struct WebviewRemovePlugin;

impl Plugin for WebviewRemovePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_webview_remove);
    }
}

fn apply_webview_remove(e: On<RequestTtyWebviewRemove>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::RemovePlacements {
        pane,
        instances: e.instances.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzmux::prelude::PaneId;

    /// Asserts that a webview-remove request for a pane entity becomes
    /// a `RemovePlacements` command carrying the same instance list.
    ///
    /// Case: one of two registrations on a pane is unregistered while
    /// both of its views are mounted.
    #[test]
    fn webview_remove_requests_become_remove_placements_commands() {
        let (mut app, commands) = app_with_connection(WebviewRemovePlugin);
        let pane = spawn_pane(&mut app, PaneId(6));
        let a: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        app.world_mut().trigger(RequestTtyWebviewRemove {
            terminal: pane,
            instances: vec![a],
        });
        let sent = sent(&commands);
        let [
            OrzmuxCommand::RemovePlacements {
                pane: PaneId(6),
                instances,
            },
        ] = sent.as_slice()
        else {
            panic!("expected one RemovePlacements, got {sent:?}");
        };
        assert_eq!(instances, &vec![a]);
    }
}
