//! `EntityEvent` types this crate triggers on terminal entities.
//!
//! Frame events (`FrameSnapshot`, `FrameDelta`) come from
//! `bevy_terminal_render::schema` and are emitted via
//! `commands.trigger(FrameSnapshot { entity, .. })` — the
//! `#[event_target] entity` field routes the trigger to the
//! correct observer.

use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;

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
