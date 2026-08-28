//! Outbound `Tty*Signal` `EntityEvent` types for terminal entities,
//! drained from the VT (`TtyBellSignal`, `TtyTitleChangedSignal`,
//! `TtyTitleResetSignal`, `TtyClipboardStoreSignal`, `TtyCwdChangedSignal`,
//! `TtyApcWebviewSignal`, `TtyWebviewEvictedSignal`, `TtyModeChangedSignal`, `TtyChildExitSignal`,
//! `TtyFrameSignal`).
//! Inbound requests fired by the host UI live in `requests.rs`.

use crate::OrzmaTtyHandle;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::prelude::*;
use orzma_tty::prelude::*;
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

/// Fired for an APC webview mount/unmount request from the PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyApcWebviewSignal {
    #[event_target]
    pub terminal: Entity,
    /// The mount/unmount verb parsed from the APC payload.
    pub verb: ApcWebviewVerb,
    /// The VT-minted placement id; `Some` only for an accepted `Mount`.
    pub placement: Option<PlacementId>,
}

/// Fired when the VT evicts placements on its own authority (history
/// trim, alternate-screen teardown); consumers despawn the matching
/// webviews by id and ignore unknown ids.
#[derive(EntityEvent, Debug, Clone)]
pub struct TtyWebviewEvictedSignal {
    #[event_target]
    pub terminal: Entity,
    pub placements: Vec<PlacementId>,
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

pub(crate) struct OrzmaTtySignalPlugin;

impl Plugin for OrzmaTtySignalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, pump_terminals);
    }
}

fn pump_terminals(mut commands: Commands, mut terms: Query<(Entity, &mut OrzmaTtyHandle)>) {
    for (terminal, mut term) in terms.iter_mut() {
        let o = term.pump();
        for signal in o.signals {
            match signal {
                TtySignal::ChildExit { code } => commands.trigger(TtyChildExitSignal {
                    entity: terminal,
                    code,
                }),
                TtySignal::Vt(signal) => trigger_vt_signal(&mut commands, terminal, signal),
            }
        }
        if let Some(frame) = o.frame {
            commands.trigger(TtyFrameSignal { terminal, frame });
        }
    }
}

fn trigger_vt_signal(commands: &mut Commands, terminal: Entity, signal: VtSignal) {
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
        VtSignal::WebviewApc { verb, placement } => commands.trigger(TtyApcWebviewSignal {
            terminal,
            verb,
            placement,
        }),
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
