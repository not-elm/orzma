//! Inbound request `EntityEvent`s and `Event`s: commands the host UI
//! fires at a terminal entity or at the backend's active pane, as
//! opposed to the outbound `Tty*Signal`s in `signals.rs` that are
//! drained FROM the backend.

use crate::requests::{
    copy::CopyPlugin, key_input::KeyInputPlugin, mouse_input::MouseInputPlugin,
    pane::PaneActionPlugin, paste::PastePlugin, scroll::ScrollPlugin, selection::SelectionPlugin,
    vi_mode::ViModePlugin, vi_motion::ViMotionPlugin, webview_remove::WebviewRemovePlugin,
};
use bevy::prelude::*;

mod copy;
mod key_input;
mod mouse_input;
mod pane;
mod paste;
mod scroll;
mod selection;
mod vi_mode;
mod vi_motion;
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
            WebviewRemovePlugin,
        ));
    }
}

/// Test-only fixtures shared by every request observer's tests: a
/// detached [`MuxClient`]-backed app plus helpers to spawn a mirrored
/// pane entity and drain what the observer sent.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::{MuxConnection, MuxPane, layout::CurrentLayout, registry::PaneRegistry};
    use bevy::prelude::*;
    use crossbeam_channel::Receiver;
    use orzma_mux::prelude::{CommandSeq, MuxClient, MuxCommand, PaneId};

    /// An app with a detached client; returns the backend's command end.
    pub(crate) fn app_with_connection(
        plugin: impl Plugin,
    ) -> (App, Receiver<(CommandSeq, MuxCommand)>) {
        let (client, _events, commands) = MuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(plugin)
            .init_resource::<PaneRegistry>()
            .init_resource::<CurrentLayout>()
            .insert_resource(MuxConnection(client));
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
