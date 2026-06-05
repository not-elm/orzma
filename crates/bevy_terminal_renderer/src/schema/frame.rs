use bevy::ecs::{entity::Entity, event::EntityEvent};
use ozmux_vt::frame::{FrameDelta, FrameSnapshot};

/// Bevy `EntityEvent` wrapping a pure `FrameSnapshot` for a specific terminal entity.
#[derive(Debug, Clone, EntityEvent)]
pub struct TerminalSnapshot {
    /// The terminal entity this snapshot belongs to.
    #[event_target]
    pub entity: Entity,
    /// The full viewport snapshot.
    pub snapshot: FrameSnapshot,
}

/// Bevy `EntityEvent` wrapping a pure `FrameDelta` for a specific terminal entity.
#[derive(Debug, Clone, EntityEvent)]
pub struct TerminalDelta {
    /// The terminal entity this delta belongs to.
    #[event_target]
    pub entity: Entity,
    /// The incremental delta.
    pub delta: FrameDelta,
}
