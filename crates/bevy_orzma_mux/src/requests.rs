//! Inbound request `EntityEvent`s and `Event`s: commands the host UI
//! fires at a terminal entity or at the backend's active pane, as
//! opposed to the outbound `Tty*Signal`s in `signals.rs` that are
//! drained FROM the backend.

use crate::requests::{
    copy::CopyPlugin, key_input::KeyInputPlugin, mouse_input::MouseInputPlugin,
    pane::PaneActionPlugin, paste::PastePlugin, scroll::ScrollPlugin, selection::SelectionPlugin,
    vi_mode::ViModePlugin, vi_motion::ViMotionPlugin, webview_mount::WebviewMountPlugin,
    webview_remove::WebviewRemovePlugin,
};
use crate::{MuxConnection, MuxPane};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use orzma_mux::prelude::{CommandSeq, MuxCommand, PaneId};

mod copy;
mod key_input;
mod mouse_input;
mod pane;
mod paste;
mod scroll;
mod selection;
mod vi_mode;
mod vi_motion;
mod webview_mount;
mod webview_remove;

pub use copy::RequestTtyCopySelection;
pub use key_input::{RequestActiveKeyInput, RequestTtyKeyInput};
pub use mouse_input::RequestTtyMouseInput;
pub use pane::{PaneAction, RequestPaneAction};
pub use paste::{RequestActivePaste, RequestTtyPaste};
pub use scroll::RequestTtyScroll;
pub use selection::{
    CellSide, GridPoint, RequestTtySelectionClear, RequestTtySelectionKindChange,
    RequestTtySelectionStart, RequestTtySelectionStartAtViCursor, RequestTtySelectionUpdate,
    SelectionKind,
};
pub use vi_mode::{RequestTtyViMode, ViModeSwitch};
pub use vi_motion::{RequestTtyViMotion, ViMotion};
pub use webview_mount::RequestTtyWebviewMount;
pub use webview_remove::RequestTtyWebviewRemove;

pub(crate) struct OrzmaEventRequestPlugin;

impl Plugin for OrzmaEventRequestPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            CopyPlugin,
            KeyInputPlugin,
            MouseInputPlugin,
            PaneActionPlugin,
            PastePlugin,
            ScrollPlugin,
            SelectionPlugin,
            ViModePlugin,
            ViMotionPlugin,
            WebviewMountPlugin,
            WebviewRemovePlugin,
        ));
    }
}

/// Sends a pane-addressed command for a terminal entity: the one place
/// that maps an entity to its `PaneId` and drops requests aimed at an
/// entity that is not (or no longer) a pane.
#[derive(SystemParam)]
pub(crate) struct PaneSender<'w, 's> {
    connection: Res<'w, MuxConnection>,
    panes: Query<'w, 's, &'static MuxPane>,
}

impl PaneSender<'_, '_> {
    /// Sends the command `build` makes for `entity`'s pane, returning its
    /// sequence number, or `None` (nothing sent) when `entity` is not a
    /// pane.
    pub(crate) fn send_for(
        &self,
        entity: Entity,
        build: impl FnOnce(PaneId) -> MuxCommand,
    ) -> Option<CommandSeq> {
        let pane = self.panes.get(entity).ok()?;
        Some(self.connection.0.send(build(pane.0)))
    }
}

/// Test-only fixtures shared by this crate's tests: a detached
/// [`MuxClient`]-backed app plus helpers to spawn a mirrored pane entity
/// and drain what an observer sent.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::{MuxConnection, MuxPane, layout::CurrentLayout, registry::PaneRegistry};
    use bevy::prelude::*;
    use crossbeam_channel::{Receiver, Sender};
    use orzma_mux::prelude::{CommandSeq, MuxClient, MuxCommand, MuxEvent, PaneId};

    /// An app with a detached client; returns the backend's ends of both
    /// channels.
    pub(crate) fn app_with_channels(
        plugin: impl Plugin,
    ) -> (App, Sender<MuxEvent>, Receiver<(CommandSeq, MuxCommand)>) {
        let (client, events, commands) = MuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(plugin)
            .init_resource::<PaneRegistry>()
            .init_resource::<CurrentLayout>()
            .insert_resource(MuxConnection(client));
        (app, events, commands)
    }

    /// An app with a detached client; returns the backend's command end.
    pub(crate) fn app_with_connection(
        plugin: impl Plugin,
    ) -> (App, Receiver<(CommandSeq, MuxCommand)>) {
        let (app, _events, commands) = app_with_channels(plugin);
        (app, commands)
    }

    /// Spawns a pane entity mirroring `pane` and registers it.
    pub(crate) fn spawn_pane(app: &mut App, pane: PaneId) -> Entity {
        let entity = app.world_mut().spawn(MuxPane(pane)).id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .panes
            .insert(pane, entity);
        entity
    }

    /// The commands sent so far, in order.
    pub(crate) fn sent(commands: &Receiver<(CommandSeq, MuxCommand)>) -> Vec<MuxCommand> {
        commands.try_iter().map(|(_, c)| c).collect()
    }
}
