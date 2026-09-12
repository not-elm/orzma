//! The outbound signals the drain turns the backend's events into: one
//! `EntityEvent` per drained `VtSignal`, plus the backend's answer to a
//! copy request.

use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::prelude::*;
use orzma_vt::prelude::*;
use std::path::PathBuf;

/// Fired when a terminal requests an audible bell. Delivery is
/// best-effort: a consumer never back-pressures the terminal that rang
/// it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyBellSignal {
    #[event_target]
    pub terminal: Entity,
}

/// Fired when the OSC terminal title changes.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyTitleChangedSignal {
    #[event_target]
    pub terminal: Entity,
    pub title: String,
}

/// Fired when the OSC terminal title resets.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyTitleResetSignal {
    #[event_target]
    pub terminal: Entity,
}

/// Fired for the mode flags that transitioned since the previous drain,
/// as mode names (e.g. "alt-screen"). [`OrzmaVt`] never raises it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyModeChangedSignal {
    #[event_target]
    pub entity: Entity,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Fired when the application copies data to the system clipboard via
/// OSC 52. [`OrzmaVt`] never raises it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyClipboardStoreSignal {
    #[event_target]
    pub terminal: Entity,
    pub content: String,
}

/// Fired exactly once when a pane closes. `code` is the shell's exit
/// code, and `None` when the GUI killed the pane or the `wait` itself
/// failed.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyChildExitSignal {
    #[event_target]
    pub entity: Entity,
    pub code: Option<i32>,
}

/// Fired when a terminal reports a new current working directory via
/// OSC 7, carrying the absolute path parsed from the URI.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyCwdChangedSignal {
    #[event_target]
    pub terminal: Entity,
    pub path: PathBuf,
}

/// Fired for a webview the PTY mounted inline and the VT registered.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyWebviewMountSignal {
    #[event_target]
    pub terminal: Entity,
    /// The host-minted instance this mount registered.
    pub instance: InstanceId,
    /// The cell rectangle the mount reserved.
    pub size: PlacementSize,
}

/// Fired for a mount the VT refused because the placement cap was full;
/// nothing was registered, so consumers only report it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyWebviewMountRejectedSignal {
    #[event_target]
    pub terminal: Entity,
    /// The instance the refused mount named.
    pub instance: InstanceId,
}

/// Fired for webview placements the PTY unmounted.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyWebviewUnmountSignal {
    #[event_target]
    pub terminal: Entity,
    /// The instance to unmount; `None` unmounts every placement.
    pub instance: Option<InstanceId>,
}

/// Fired when the VT drops placements without the host naming them
/// (history trim, reset, alternate-screen teardown, resize); consumers
/// despawn the matching webviews by id and ignore unknown ids.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyWebviewEvictedSignal {
    #[event_target]
    pub terminal: Entity,
    /// The instances the VT evicted. It has already dropped them, so no
    /// removal needs to be sent back.
    pub placements: Vec<InstanceId>,
}

/// Fired when the terminal emits a frame; a full repaint carries every
/// viewport row, and the changed-only sections (`placements`,
/// `palette`) are `None` when unchanged since the previous frame.
#[derive(EntityEvent, Debug)]
pub struct TtyFrameSignal {
    #[event_target]
    pub terminal: Entity,
    /// The emitted frame.
    pub frame: Frame,
}

/// The backend's answer to a `RequestTtyCopySelection`: the selected
/// text (`None` when the pane was gone or the selection empty).
#[derive(Event, Debug, Clone)]
pub struct TtySelectionTextSignal {
    pub text: Option<String>,
}

pub(crate) fn trigger_vt_signal(commands: &mut Commands, terminal: Entity, signal: VtSignal) {
    match signal {
        VtSignal::Bell => commands.trigger(TtyBellSignal { terminal }),
        VtSignal::Title(title) => commands.trigger(TtyTitleChangedSignal { terminal, title }),
        VtSignal::ResetTitle => commands.trigger(TtyTitleResetSignal { terminal }),
        VtSignal::Clipboard { content } => {
            commands.trigger(TtyClipboardStoreSignal { terminal, content })
        }
        VtSignal::CurrentDir(path_buf) => commands.trigger(TtyCwdChangedSignal {
            terminal,
            path: path_buf,
        }),
        VtSignal::WebviewMount { instance, size } => commands.trigger(TtyWebviewMountSignal {
            terminal,
            instance,
            size,
        }),
        VtSignal::WebviewMountRejected { instance } => {
            commands.trigger(TtyWebviewMountRejectedSignal { terminal, instance })
        }
        VtSignal::WebviewUnmount { instance } => {
            commands.trigger(TtyWebviewUnmountSignal { terminal, instance })
        }
        VtSignal::WebviewEvicted { placements } => commands.trigger(TtyWebviewEvictedSignal {
            terminal,
            placements,
        }),
        VtSignal::ModeChange { added, removed } => commands.trigger(TtyModeChangedSignal {
            entity: terminal,
            added: added.into_iter().map(String::from).collect(),
            removed: removed.into_iter().map(String::from).collect(),
        }),
    }
}
