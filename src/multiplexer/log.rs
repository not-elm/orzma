//! Layout-change logging system. Watches `Changed<LayoutCells>` on
//! Session entities and logs a human-readable summary of the tree.
//! `OzmuxLayoutLogPlugin` registers the system in `Update`.
//!
//! Full rendering logic is deferred — see the TODO below.

use bevy::prelude::*;
use ozmux_multiplexer::{LayoutCells, SessionMarker};

/// Bevy Plugin that registers `log_layout_changes` in the `Update`
/// schedule behind `Changed<LayoutCells>` so it fires only on layout
/// mutations.
pub struct OzmuxLayoutLogPlugin;

impl Plugin for OzmuxLayoutLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, log_layout_changes);
    }
}

fn log_layout_changes(
    sessions: Query<
        (Entity, &Name),
        (With<SessionMarker>, Changed<LayoutCells>),
    >,
) {
    // TODO: re-enable full layout-change logging in Task 16.
    // The old render_tree formatter read from MultiplexerService (removed).
    // A new formatter that walks LayoutCells + Children queries is needed.
    for (entity, name) in sessions.iter() {
        tracing::info!(
            target: "ozmux_gui::layout",
            ?entity,
            session = %name,
            "layout changed",
        );
    }
}
