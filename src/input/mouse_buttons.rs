//! Bevy plugin that drives mouse-button selection. Reads
//! `MouseButtonInput` and `CursorMoved` events, hit-tests against
//! activity hosts, builds `ButtonEvent`s, dispatches them through
//! `bevy_terminal::ButtonAction::route`, and applies the result.
//!
//! State is owned by the `MouseSelectionState` resource — see spec
//! §6.

use bevy::prelude::*;
use bevy_terminal::{CellCoord, SelectionType, Side};
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

/// Hit-tests `cursor_phys_px` against all `ActivityHostNode` entities
/// and returns `(entity, local_phys_px)` for the first pane that
/// contains the cursor. `local_phys_px` is in pane-local pixels with
/// origin at the top-left corner of the node (i.e., `(0, 0)` is the
/// top-left, `(size.x, size.y)` is the bottom-right).
///
/// `cursor_phys_px` is in physical (DPR-scaled) pixels — the caller
/// must convert from `Window::cursor_position()` (logical) by
/// multiplying by `Window::scale_factor()` first.
#[allow(dead_code)] // wired into dispatch_mouse_buttons in subsequent tasks
pub(crate) fn resolve_pane_at_phys(
    hosts: &Query<
        (
            Entity,
            &bevy::ui::ComputedNode,
            &bevy::ui::UiGlobalTransform,
        ),
        With<crate::ui::ActivityHostNode>,
    >,
    cursor_phys_px: Vec2,
) -> Option<(Entity, Vec2)> {
    for (entity, node, transform) in hosts.iter() {
        if !node.contains_point(*transform, cursor_phys_px) {
            continue;
        }
        // NOTE: normalize_point returns None if the affine transform is
        // degenerate (zero-size node or non-invertible). contains_point
        // returning true normally implies Some here, but skip defensively
        // to avoid an unwrap on the degenerate case.
        let Some(normalized) = node.normalize_point(*transform, cursor_phys_px) else {
            continue;
        };
        let local = (normalized + Vec2::splat(0.5)) * node.size;
        return Some((entity, local));
    }
    None
}

/// Projects a pane-local physical-pixel point onto 1-indexed
/// `(col, row, side)`. Clamps `col` to `1..=cols` and `row` to
/// `1..=rows`. `cell_w_phys` / `cell_h_phys` are the physical-pixel
/// cell pitch from `TerminalCellMetricsResource`.
pub(crate) fn cell_at_local(
    local_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    cols: u16,
    rows: u16,
) -> (u32, u32, Side) {
    let col_f = (local_phys.x / cell_w_phys).max(0.0);
    let row_f = (local_phys.y / cell_h_phys).max(0.0);
    let col = (col_f.floor() as u32 + 1).min(cols as u32).max(1);
    let row = (row_f.floor() as u32 + 1).min(rows as u32).max(1);
    let frac_x = col_f - col_f.floor();
    let side = if frac_x < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (col, row, side)
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

    #[test]
    fn cell_at_local_projects_to_one_indexed_coords_and_side() {
        // 10x10 physical px cell. local (15, 25) → col 2 (15/10=1.5→floor 1, +1=2), row 3 (25/10=2.5→floor 2, +1=3).
        // frac_x = 0.5 → Side::Right.
        let (col, row, side) = super::cell_at_local(Vec2::new(15.0, 25.0), 10.0, 10.0, 80, 24);
        assert_eq!(col, 2);
        assert_eq!(row, 3);
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn cell_at_local_left_half_returns_side_left() {
        // local (2, 5): col 1, row 1; frac_x = 0.2 < 0.5 → Side::Left.
        let (_col, _row, side) = super::cell_at_local(Vec2::new(2.0, 5.0), 10.0, 10.0, 80, 24);
        assert_eq!(side, Side::Left);
    }

    #[test]
    fn cell_at_local_clamps_to_grid() {
        // Local position past grid bounds clamps to (cols, rows).
        let (col, row, _side) =
            super::cell_at_local(Vec2::new(10_000.0, 10_000.0), 10.0, 10.0, 80, 24);
        assert_eq!(col, 80);
        assert_eq!(row, 24);
    }
}
