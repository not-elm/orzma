//! `TmuxPane` pointer hit-testing. Cell geometry lives in `crate::surface_geom`.

use crate::surface_geom::phys_to_pane_local;
use bevy::ecs::entity::Entity;
use bevy::ecs::system::Query;
use bevy::math::Vec2;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use ozmux_tmux::{PaneId, TmuxPane};

/// The first `TmuxPane` under `cursor_phys_px`, with the pointer in pane-local
/// physical px. Skips panes without a laid-out node.
pub(crate) fn tmux_pane_at_phys(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys_px: Vec2,
) -> Option<(Entity, PaneId, Vec2)> {
    for (entity, pane, node, transform) in panes.iter() {
        if !node.contains_point(*transform, cursor_phys_px) {
            continue;
        }
        let Some(local) = phys_to_pane_local(node, transform, cursor_phys_px) else {
            continue;
        };
        return Some((entity, pane.id, local));
    }
    None
}
