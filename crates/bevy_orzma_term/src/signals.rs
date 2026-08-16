//! Outbound `Term*Signal` `EntityEvent` types for terminal entities,
//! drained from the VT (`TermBellSignal`, `TermTitleChangedSignal`,
//! `TermTitleResetSignal`, `TermClipboardStoreSignal`, `TermCwdChangedSignal`,
//! `TermApcWebviewSignal`, `TermWebviewEvictedSignal`, `TermModeChangedSignal`, `TermChildExitSignal`,
//! `TermFrameSnapshotSignal`, `TermFrameDeltaSignal`).
//! Inbound requests fired by the host UI live in `requests.rs`.

use crate::OrzmaTermHandle;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::prelude::*;
use orzma_term::prelude::*;
use orzma_vt::prelude::*;
use std::path::PathBuf;

/// Fired when alacritty raises `Event::Bell`.
/// Best-effort — no back-pressure observability (control channel is unbounded).
#[derive(EntityEvent, Debug, Clone)]
pub struct TermBellSignal {
    #[event_target]
    pub terminal: Entity,
}

/// Fired when the OSC terminal title changes.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermTitleChangedSignal {
    #[event_target]
    pub terminal: Entity,
    pub title: String,
}

/// Fired when the OSC terminal title resets.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermTitleResetSignal {
    #[event_target]
    pub terminal: Entity,
}

/// Fired when tracked `TermMode` flags transition between coalescer
/// emit cycles.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermModeChangedSignal {
    #[event_target]
    pub entity: Entity,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Fired when alacritty raises `Event::ClipboardStore`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermClipboardStoreSignal {
    #[event_target]
    pub terminal: Entity,
    pub content: String,
}

/// Fired exactly once when the child shell process exits.
/// `code` is `None` if the `wait` itself failed.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermChildExitSignal {
    #[event_target]
    pub entity: Entity,
    pub code: Option<i32>,
}

/// Fired when a terminal reports a new current working directory via OSC 7.
/// Targets the terminal host entity; carries the validated absolute path.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermCwdChangedSignal {
    #[event_target]
    pub terminal: Entity,
    pub path: PathBuf,
}

/// Fired for an APC webview mount/unmount request from the PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct TermApcWebviewSignal {
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
pub struct TermWebviewEvictedSignal {
    #[event_target]
    pub terminal: Entity,
    pub placements: Vec<PlacementId>,
}

/// Fired when the terminal emits a full-repaint snapshot frame.
#[derive(EntityEvent, Debug)]
pub struct TermFrameSnapshotSignal {
    #[event_target]
    pub terminal: Entity,
    /// The emitted snapshot.
    pub frame: FrameSnapshot,
}

/// Fired when the terminal emits a differential frame.
#[derive(EntityEvent, Debug)]
pub struct TermFrameDeltaSignal {
    #[event_target]
    pub terminal: Entity,
    /// The emitted delta.
    pub delta: FrameDelta,
}

pub(crate) struct OrzmaTermSignalPlugin;

impl Plugin for OrzmaTermSignalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, pump_terminals);
    }
}

fn pump_terminals(mut commands: Commands, mut terms: Query<(Entity, &mut OrzmaTermHandle)>) {
    for (terminal, mut term) in terms.iter_mut() {
        let o = term.pump();
        for signal in o.signals {
            match signal {
                TermSignal::ChildExit { code } => commands.trigger(TermChildExitSignal {
                    entity: terminal,
                    code,
                }),
                TermSignal::Vt(signal) => trigger_vt_signal(&mut commands, terminal, signal),
            }
        }
        match o.frame {
            Some(Frame::Snapshot(frame)) => {
                commands.trigger(TermFrameSnapshotSignal { terminal, frame })
            }
            Some(Frame::Delta(delta)) => commands.trigger(TermFrameDeltaSignal { terminal, delta }),
            None => {}
        }
    }
}

fn trigger_vt_signal(commands: &mut Commands, terminal: Entity, signal: VtSignal) {
    match signal {
        VtSignal::Bell => commands.trigger(TermBellSignal { terminal }),
        VtSignal::Title(title) => commands.trigger(TermTitleChangedSignal { terminal, title }),
        VtSignal::ResetTitle => commands.trigger(TermTitleResetSignal { terminal }),
        VtSignal::Clipboard { content } => {
            commands.trigger(TermClipboardStoreSignal { terminal, content })
        }
        VtSignal::CurrentDir(path_buf) => commands.trigger(TermCwdChangedSignal {
            terminal,
            path: path_buf,
        }),
        VtSignal::ApcWebview { verb, placement } => commands.trigger(TermApcWebviewSignal {
            terminal,
            verb,
            placement,
        }),
        VtSignal::WebviewEvicted { placements } => commands.trigger(TermWebviewEvictedSignal {
            terminal,
            placements,
        }),
        VtSignal::ModeChange { added, removed } => commands.trigger(TermModeChangedSignal {
            entity: terminal,
            added: added.into_iter().map(String::from).collect(),
            removed: removed.into_iter().map(String::from).collect(),
        }),
    }
}
