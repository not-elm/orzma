//! `EntityEvent` types for terminal entities — both outbound events
//! triggered by this crate (`TerminalBell`, `TerminalTitleChanged`,
//! `TerminalModeChanged`, `TerminalClipboardStore`, `TerminalChildExit`,
//! `TerminalCurrentDir`) and inbound commands triggered by the host UI
//! and observed by `TerminalHandlePlugin` (`TerminalKeyInput`).
//!
//! Frame events (`FrameSnapshot`, `FrameDelta`) come from
//! `orzma_tty_renderer::schema` and are emitted via
//! `commands.trigger(FrameSnapshot { entity, .. })` — the
//! `#[event_target] entity` field routes the trigger to the
//! correct observer.

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
pub struct NotifyTermBell {
    #[event_target]
    pub terminal: Entity,
}

/// Fired when the OSC terminal title changes.
#[derive(EntityEvent, Debug, Clone)]
pub struct NotifyTermTitleChanged {
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
pub struct NotifyTermModeChanged {
    #[event_target]
    pub entity: Entity,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Fired when alacritty raises `Event::ClipboardStore`.
#[derive(EntityEvent, Debug, Clone)]
pub struct NotifyTermClipboardStore {
    #[event_target]
    pub terminal: Entity,
    pub content: String,
}

/// Fired exactly once when the child shell process exits.
/// `code` is `None` if the `wait` itself failed.
#[derive(EntityEvent, Debug, Clone)]
pub struct NotifyTermChildExit {
    #[event_target]
    pub entity: Entity,
    pub code: Option<i32>,
}

/// Fired when a terminal reports a new current working directory via OSC 7.
/// Targets the terminal host entity; carries the validated absolute path.
#[derive(EntityEvent, Debug, Clone)]
pub struct NotifyTermCwdChanged {
    #[event_target]
    pub terminal: Entity,
    pub path: PathBuf,
}

/// An OSC-driven webview mount/unmount request from a terminal surface's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestApcWebview {
    #[event_target]
    pub terminal: Entity,
    /// The inline mount/unmount verb parsed from the OSC 5379 payload.
    pub verb: ApcWebviewVerb,
    /// Anchor metadata for `Mount` (absolute line + column + frame seq);
    /// `None` for every other verb.
    pub anchor: Option<InlineAnchor>,
}

/// Fired by the host UI to forward a key press to a specific Terminal
/// Surface entity. The observer registered by `TerminalHandlePlugin`
/// encodes the key using the entity's `Term::mode()` and writes the
/// resulting VT bytes to the PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTerminalKeyInput {
    #[event_target]
    pub entity: Entity,
    pub key: TerminalKey,
    pub modifiers: TerminalModifiers,
}

pub(crate) struct OrzmaTermSignalPlugin;

impl Plugin for OrzmaTermSignalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, signal_terminal_events);
    }
}

fn signal_terminal_events(
    mut commands: Commands,
    mut terms: Query<(Entity, &mut OrzmaTermHandle)>,
) {
    for (terminal, mut term) in terms.iter_mut() {
        for e in term.vt_mut().drain_signals() {
            match e {
                TermSignal::Bell => commands.trigger(NotifyTermBell { terminal }),
                TermSignal::Title(title) => {
                    commands.trigger(NotifyTermTitleChanged { terminal, title })
                }
                TermSignal::ResetTitle => commands.trigger(TermTitleResetSignal { terminal }),
                TermSignal::Clipboard { content } => {
                    commands.trigger(NotifyTermClipboardStore { terminal, content })
                }
                TermSignal::CurrentDir(path_buf) => commands.trigger(NotifyTermCwdChanged {
                    terminal,
                    path: path_buf,
                }),
                TermSignal::ApcWebview { verb, anchor } => commands.trigger(RequestApcWebview {
                    terminal,
                    verb,
                    anchor,
                }),
            }
        }
    }
}
