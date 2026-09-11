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
use crate::{OrzmuxConnection, OrzmuxPane};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use orzmux::prelude::{CommandSeq, OrzmuxCommand, PaneId};

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
///
/// It reads `OrzmuxConnection`, so every observer that takes it must be
/// registered with `run_if(resource_exists::<OrzmuxConnection>)`.
#[derive(SystemParam)]
pub(crate) struct PaneSender<'w, 's> {
    connection: Res<'w, OrzmuxConnection>,
    panes: Query<'w, 's, &'static OrzmuxPane>,
}

impl PaneSender<'_, '_> {
    /// Sends the command `build` makes for `entity`'s pane, returning its
    /// sequence number, or `None` (nothing sent) when `entity` is not a
    /// pane.
    pub(crate) fn send_for(
        &self,
        entity: Entity,
        build: impl FnOnce(PaneId) -> OrzmuxCommand,
    ) -> Option<CommandSeq> {
        let pane = self.panes.get(entity).ok()?;
        Some(self.connection.0.send(build(pane.0)))
    }
}

/// Test-only fixtures shared by this crate's tests: a detached
/// [`OrzmuxClient`]-backed app plus helpers to spawn a mirrored pane entity
/// and drain what an observer sent.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::{OrzmuxConnection, OrzmuxPane, layout::CurrentLayout, registry::PaneRegistry};
    use bevy::prelude::*;
    use crossbeam_channel::{Receiver, Sender};
    use orzmux::prelude::{CommandSeq, OrzmuxClient, OrzmuxCommand, OrzmuxEvent, PaneId};

    /// An app with a detached client; returns the backend's ends of both
    /// channels.
    pub(crate) fn app_with_channels(
        plugin: impl Plugin,
    ) -> (
        App,
        Sender<OrzmuxEvent>,
        Receiver<(CommandSeq, OrzmuxCommand)>,
    ) {
        let (client, events, commands) = OrzmuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(plugin)
            .init_resource::<PaneRegistry>()
            .init_resource::<CurrentLayout>()
            .insert_resource(OrzmuxConnection(client));
        (app, events, commands)
    }

    /// An app with a detached client; returns the backend's command end.
    pub(crate) fn app_with_connection(
        plugin: impl Plugin,
    ) -> (App, Receiver<(CommandSeq, OrzmuxCommand)>) {
        let (app, _events, commands) = app_with_channels(plugin);
        (app, commands)
    }

    /// Spawns a pane entity mirroring `pane` and registers it.
    pub(crate) fn spawn_pane(app: &mut App, pane: PaneId) -> Entity {
        let entity = app.world_mut().spawn(OrzmuxPane(pane)).id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .panes
            .insert(pane, entity);
        entity
    }

    /// The commands sent so far, in order.
    pub(crate) fn sent(commands: &Receiver<(CommandSeq, OrzmuxCommand)>) -> Vec<OrzmuxCommand> {
        commands.try_iter().map(|(_, c)| c).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, spawn_pane};
    use orzma_tty::prelude::{
        CellCoord, KeyText, MouseButton, MouseReport, MouseReportKind, ProtocolModifiers,
        TerminalKey, TerminalModifiers,
    };
    use orzma_vt::prelude::{GridColumn, GridLine, InstanceId, PlacementSize, ScreenLine, Scroll};
    use orzmux::prelude::PaneId;

    /// Asserts that no request observer runs once the connection is
    /// removed, so none of them panics.
    ///
    /// Case: the backend thread has died, and the user keeps typing,
    /// clicking, and pasting before `AppExit` takes effect.
    #[test]
    fn no_request_observer_runs_without_a_connection() {
        let (mut app, _commands) = app_with_connection(OrzmaEventRequestPlugin);
        let pane = spawn_pane(&mut app, PaneId(1));
        app.world_mut().remove_resource::<OrzmuxConnection>();
        let key = TerminalKey::Character(KeyText::new("x").unwrap());
        let cell = GridPoint {
            line: GridLine(0),
            column: GridColumn(0),
        };
        let instance: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");

        let world = app.world_mut();
        world.trigger(RequestTtyKeyInput {
            terminal: pane,
            key: key.clone(),
            modifiers: TerminalModifiers::default(),
        });
        world.trigger(RequestActiveKeyInput {
            key,
            modifiers: TerminalModifiers::default(),
        });
        world.trigger(RequestTtyPaste {
            terminal: pane,
            text: "x".into(),
        });
        world.trigger(RequestActivePaste { text: "x".into() });
        world.trigger(RequestTtyCopySelection { terminal: pane });
        world.trigger(RequestTtyMouseInput {
            terminal: pane,
            mouse: MouseReport {
                button: MouseButton::Left,
                kind: MouseReportKind::Press,
                cell: CellCoord { col: 1, row: 1 },
                mods: ProtocolModifiers::default(),
            },
        });
        world.trigger(RequestTtyScroll {
            terminal: pane,
            scroll: Scroll::Delta(1),
        });
        world.trigger(RequestTtySelectionStart {
            terminal: pane,
            cell,
            side: CellSide::Left,
            kind: SelectionKind::Simple,
        });
        world.trigger(RequestTtySelectionUpdate {
            terminal: pane,
            cell,
            side: CellSide::Right,
        });
        world.trigger(RequestTtySelectionClear { terminal: pane });
        world.trigger(RequestTtyWebviewMount {
            terminal: pane,
            instance,
            row: ScreenLine(0),
            column: GridColumn(0),
            size: PlacementSize { rows: 1, cols: 1 },
        });
        world.trigger(RequestTtyWebviewRemove {
            terminal: pane,
            instances: vec![instance],
        });
        world.trigger(RequestPaneAction {
            action: PaneAction::Kill,
        });
        world.trigger(RequestPaneAction {
            action: PaneAction::Select(pane),
        });
        app.update();
    }
}
