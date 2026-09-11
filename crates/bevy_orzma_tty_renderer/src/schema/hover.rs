//! Global hover state for the hyperlink under the pointer.

use crate::schema::HyperlinkId;
use bevy::ecs::entity::Entity;
use bevy::ecs::resource::Resource;

/// Pointer hover state that drives the hyperlink underline accent.
/// Exactly one cell can be hovered at a time across all panes.
#[derive(Resource, Default, Debug, Clone)]
pub struct HyperlinkHoverState {
    /// Surface-host entity the cursor is over, or `None` when the cursor
    /// is outside every pane.
    pub entity: Option<Entity>,
    /// Hovered wire id, or `None` when the cursor is over an unlinked
    /// cell; meaningful only when `entity` is `Some`.
    pub hyperlink_id: Option<HyperlinkId>,
    /// Whether the activation modifier (Cmd on macOS, Ctrl elsewhere) is
    /// held. It drives the shader's `hover_active` uniform.
    pub modifier_held: bool,
}
