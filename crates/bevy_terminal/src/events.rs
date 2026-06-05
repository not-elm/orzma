//! `EntityEvent` types for terminal entities — both outbound events
//! triggered by this crate (`TerminalBell`, `TerminalTitleChanged`,
//! `TerminalModeChanged`, `TerminalClipboardStore`, `TerminalChildExit`,
//! `TerminalCurrentDir`) and inbound commands triggered by the host UI
//! and observed by `TerminalHandlePlugin` (`TerminalKeyInput`).
//!
//! Frame events (`TerminalSnapshot`, `TerminalDelta`) come from
//! `bevy_terminal_renderer::schema` and wrap the pure
//! `ozmux_vt::frame::{FrameSnapshot, FrameDelta}` payloads; they are
//! emitted via `commands.trigger(TerminalSnapshot { entity, snapshot })`
//! — the `#[event_target] entity` field routes the trigger to the
//! correct observer.

use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use ozmux_vt::input::{TerminalKey, TerminalModifiers};
use std::path::PathBuf;

/// Fired when alacritty raises `Event::Bell`. Best-effort — no
/// back-pressure observability (control channel is unbounded).
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalBell {
    #[event_target]
    pub entity: Entity,
}

/// Fired when the OSC terminal title changes. `title = None` after
/// `Event::ResetTitle`; `Some(s)` carries the sanitized string.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalTitleChanged {
    #[event_target]
    pub entity: Entity,
    pub title: Option<String>,
}

/// Fired when tracked `TermMode` flags transition between coalescer
/// emit cycles.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalModeChanged {
    #[event_target]
    pub entity: Entity,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Fired when alacritty raises `Event::ClipboardStore`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalClipboardStore {
    #[event_target]
    pub entity: Entity,
    pub content: String,
}

/// Fired exactly once when the child shell process exits. `code` is
/// `None` if the `wait` itself failed.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalChildExit {
    #[event_target]
    pub entity: Entity,
    pub code: Option<i32>,
}

/// Fired when a terminal reports a new current working directory via OSC 7.
/// Targets the terminal host entity; carries the validated absolute path.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalCurrentDir {
    #[event_target]
    pub entity: Entity,
    pub path: PathBuf,
}

/// Fired by the host UI to forward a key press to a specific Terminal
/// Surface entity. The observer registered by `TerminalHandlePlugin`
/// encodes the key using the entity's `Term::mode()` and writes the
/// resulting VT bytes to the PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalKeyInput {
    #[event_target]
    pub entity: Entity,
    pub key: TerminalKey,
    pub modifiers: TerminalModifiers,
}
