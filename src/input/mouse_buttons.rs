//! Bevy plugin that drives mouse-button selection. Reads
//! `MouseButtonInput` and `CursorMoved` events, hit-tests against
//! activity hosts, builds `ButtonEvent`s, dispatches them through
//! `bevy_terminal::ButtonAction::route`, and applies the result.
//!
//! State is owned by the `MouseSelectionState` resource — see spec
//! §6.

use bevy::prelude::*;
use bevy_terminal::{CellCoord, SelectionType};
use std::time::Instant;

/// Per-frame state for the mouse-selection system.
#[derive(Resource, Default)]
pub(crate) struct MouseSelectionState {
    drag: Option<ActiveDrag>,
    last_click: Option<LastClick>,
    /// Next allowed autoscroll tick. `None` outside autoscroll.
    next_autoscroll_at: Option<Instant>,
}

#[allow(dead_code)] // fields populated in subsequent tasks
struct ActiveDrag {
    entity: Entity,
    ty: SelectionType,
    anchor_cell: CellCoord,
    in_copy_mode: bool,
}

#[allow(dead_code)] // fields populated in subsequent tasks
struct LastClick {
    entity: Entity,
    cell: CellCoord,
    cursor_pos_logical_px: Vec2,
    at: Instant,
    count: u8,
}

/// Bevy plugin that registers `MouseSelectionState` and the per-frame
/// `dispatch_mouse_buttons` system in `OzmuxSystems::Input`.
pub(crate) struct MouseButtonsInputPlugin;

impl Plugin for MouseButtonsInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MouseSelectionState>().add_systems(
            Update,
            dispatch_mouse_buttons
                .in_set(crate::system_set::OzmuxSystems::Input)
                .before(crate::input::dispatch_focused_key),
        );
    }
}

/// Per-frame system entrypoint. Skeleton — Tasks 15-20 fill it in.
fn dispatch_mouse_buttons(_state: ResMut<MouseSelectionState>) {
    // Filled in by subsequent tasks.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_state_resource() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MouseButtonsInputPlugin);
        assert!(app.world().contains_resource::<MouseSelectionState>());
    }
}
