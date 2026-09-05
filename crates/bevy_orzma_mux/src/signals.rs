//! Outbound signal types the drain triggers (`TtyBellSignal`,
//! `TtyTitleChangedSignal`, `TtyTitleResetSignal`, `TtyClipboardStoreSignal`,
//! `TtyCwdChangedSignal`, `TtyWebviewMountSignal`,
//! `TtyWebviewMountRejectedSignal`, `TtyWebviewUnmountSignal`,
//! `TtyWebviewEvictedSignal`, `TtyModeChangedSignal`, `TtyChildExitSignal`,
//! `TtyFrameSignal`), plus `trigger_vt_signal`, the helper that turns one
//! drained `VtSignal` into its matching `EntityEvent`.
//! `TtySelectionTextSignal` answers a mux `CopySelection` request instead
//! of draining from a VT, so it is a plain `Event` rather than an
//! `EntityEvent`. Inbound requests fired by the host UI live in
//! `requests.rs`.

use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::prelude::*;
use orzma_mux::prelude::RequestId;
use orzma_vt::prelude::*;
use std::path::PathBuf;

/// Fired when alacritty raises `Event::Bell`.
/// Best-effort — no back-pressure observability (control channel is unbounded).
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

/// Fired when tracked `TermMode` flags transition between coalescer
/// emit cycles.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyModeChangedSignal {
    #[event_target]
    pub entity: Entity,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Fired when alacritty raises `Event::ClipboardStore`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyClipboardStoreSignal {
    #[event_target]
    pub terminal: Entity,
    pub content: String,
}

/// Fired exactly once when the child shell process exits.
/// `code` is `None` if the `wait` itself failed.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyChildExitSignal {
    #[event_target]
    pub entity: Entity,
    pub code: Option<i32>,
}

/// Fired when a terminal reports a new current working directory via OSC 7.
/// Targets the terminal host entity; carries the validated absolute path.
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
    /// The instances the VT evicted. It has already dropped them, so a
    /// consumer despawns its own side without asking for a second removal.
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
/// text of the requesting entity (`None` when the pane was gone or the
/// selection empty). A plain `Event` because `terminal` may be `None`.
#[derive(Event, Debug, Clone)]
pub struct TtySelectionTextSignal {
    pub terminal: Option<Entity>,
    pub request: RequestId,
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
