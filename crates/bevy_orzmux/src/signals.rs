//! The outbound signals the drain turns the backend's events into.

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

/// Fired when a terminal's title changes: the title its application set
/// through OSC 0 / OSC 2, trimmed, or `None` when it set a blank one or the
/// terminal reset it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyTitleSignal {
    /// The terminal whose title changed.
    #[event_target]
    pub terminal: Entity,
    /// Its new title.
    pub title: Option<String>,
}

/// Fired when the application copies data to the system clipboard via
/// OSC 52. An empty `content` leaves the clipboard holding the empty
/// string.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyClipboardStoreSignal {
    #[event_target]
    pub terminal: Entity,
    pub content: String,
}

/// Fired when the application clears the system clipboard via OSC 52.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyClipboardClearSignal {
    #[event_target]
    pub terminal: Entity,
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

/// Selected text for the clipboard: the backend's answer to a
/// `RequestTtyCopySelection` (`None` when the pane was gone or the
/// selection empty), or the text of a selection drag the user just
/// finished (never `None`).
#[derive(Event, Debug, Clone)]
pub struct TtySelectionTextSignal {
    pub text: Option<String>,
}

pub(crate) fn trigger_vt_signal(commands: &mut Commands, terminal: Entity, signal: VtSignal) {
    match signal {
        VtSignal::Bell => commands.trigger(TtyBellSignal { terminal }),
        VtSignal::Title(_) | VtSignal::ResetTitle => {
            tracing::debug!(
                ?terminal,
                "raw title signal dropped; titles arrive as PaneTitle"
            );
        }
        VtSignal::Clipboard { content } => {
            commands.trigger(TtyClipboardStoreSignal { terminal, content })
        }
        VtSignal::ClearClipboard => commands.trigger(TtyClipboardClearSignal { terminal }),
        VtSignal::CurrentDir(path_buf) => commands.trigger(TtyCwdChangedSignal {
            terminal,
            path: path_buf,
        }),
        VtSignal::WebviewMount { .. }
        | VtSignal::WebviewMountRejected { .. }
        | VtSignal::WebviewUnmount { .. }
        | VtSignal::WebviewEvicted { .. } => {
            tracing::debug!(
                ?terminal,
                "webview placement signal dropped; the webview host owns placements"
            );
        }
    }
}
