//! `RequestTtyWebviewMount`: the host-driven mount the control plane asks
//! a terminal entity to register when a program mounts over the socket
//! rather than the PTY, sent as `OrzmuxCommand::MountPlacement`.

use crate::requests::PaneSender;
use bevy::prelude::*;
use orzma_vt::prelude::{GridColumn, InstanceId, PlacementSize, ScreenLine};
use orzmux::prelude::OrzmuxCommand;

/// Fired by the control plane to register a webview placement at a
/// visible cell of the terminal, on behalf of a program whose PTY drops
/// the APC `mount` verb (ConPTY on Windows).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyWebviewMount {
    #[event_target]
    pub terminal: Entity,
    /// The host-minted instance the mount registers.
    pub instance: InstanceId,
    /// The visible row the rect's top edge sits on.
    pub row: ScreenLine,
    /// The column the rect's left edge sits on.
    pub column: GridColumn,
    /// The rect's extent in cells.
    pub size: PlacementSize,
}

pub(super) struct WebviewMountPlugin;

impl Plugin for WebviewMountPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_webview_mount);
    }
}

fn apply_webview_mount(e: On<RequestTtyWebviewMount>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::MountPlacement {
        pane,
        instance: e.instance,
        row: e.row,
        column: e.column,
        size: e.size,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzmux::prelude::PaneId;

    /// Asserts that a webview-mount request for a pane entity becomes a
    /// `MountPlacement` command carrying the same instance, cell, and size.
    ///
    /// Case: orzmd in a Windows pane draws its webview for the first time
    /// and the control plane relays its socket `mount`.
    #[test]
    fn webview_mount_requests_become_mount_placement_commands() {
        let (mut app, commands) = app_with_connection(WebviewMountPlugin);
        let pane = spawn_pane(&mut app, PaneId(6));
        let a: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        app.world_mut().trigger(RequestTtyWebviewMount {
            terminal: pane,
            instance: a,
            row: ScreenLine(1),
            column: GridColumn(2),
            size: PlacementSize { rows: 12, cols: 48 },
        });
        let sent = sent(&commands);
        let [
            OrzmuxCommand::MountPlacement {
                pane: PaneId(6),
                instance,
                row,
                column,
                size,
            },
        ] = sent.as_slice()
        else {
            panic!("expected one MountPlacement, got {sent:?}");
        };
        assert_eq!(*instance, a);
        assert_eq!(*row, ScreenLine(1));
        assert_eq!(*column, GridColumn(2));
        assert_eq!(*size, PlacementSize { rows: 12, cols: 48 });
    }
}
