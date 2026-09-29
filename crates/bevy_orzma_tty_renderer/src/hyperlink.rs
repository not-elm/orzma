//! Global hover state for the link under the pointer: an OSC 8 hyperlink
//! or a URL detected in plain text.

use bevy::ecs::entity::Entity;
use bevy::ecs::resource::Resource;
use orzma_vt::prelude::{DetectedUrl, HyperlinkId};

/// Pointer hover state that drives the hyperlink underline accent.
/// Exactly one cell can be hovered at a time across all panes.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct HyperlinkHoverState {
    /// Surface-host entity the cursor is over, or `None` when the cursor
    /// is outside every pane.
    pub entity: Option<Entity>,
    /// Hovered wire id, or `None` when the cursor is over an unlinked
    /// cell; meaningful only when `entity` is `Some`.
    pub hyperlink_id: Option<HyperlinkId>,
    /// The URL detected in the plain text under the pointer; `None` over
    /// an OSC 8 link, over text that shows no URL, or while the activation
    /// modifier is up. Meaningful only when `entity` is `Some`.
    pub detected: Option<DetectedUrl>,
    /// Whether the activation modifier (Cmd on macOS, Ctrl elsewhere) is
    /// held. It drives the shader's `hover_active` uniform.
    pub modifier_held: bool,
}
