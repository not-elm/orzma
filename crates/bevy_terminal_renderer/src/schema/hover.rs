//! Global hover state shared between the ozmux input layer (writer) and
//! the renderer's `update_terminal_material` (reader). Lives in the
//! renderer crate so the renderer does not depend on the application
//! crate.

use crate::schema::HyperlinkId;
use bevy::ecs::entity::Entity;
use bevy::ecs::resource::Resource;

/// Pointer hover state used to drive the hyperlink underline-accent and
/// the OS cursor icon. Exactly one cell can be hovered at a time across
/// all panes, so this is a global Resource rather than per-pane
/// component.
#[derive(Resource, Default, Debug, Clone)]
pub struct HyperlinkHoverState {
    /// Activity-host entity the cursor is currently over, or `None`
    /// when the cursor is outside every pane.
    pub entity: Option<Entity>,
    /// Hovered wire id; meaningful only when `entity` is `Some`. `None`
    /// when the cursor is over an unlinked cell.
    pub hyperlink_id: Option<HyperlinkId>,
    /// Activation modifier (Cmd on macOS, Ctrl elsewhere) is currently
    /// held. Drives both the cursor icon flip and the shader's
    /// `hover_active` uniform.
    pub modifier_held: bool,
}
