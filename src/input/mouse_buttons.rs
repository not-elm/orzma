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

/// Computes the click count (1, 2, or 3) for a new left-press and
/// updates `state.last_click`. Per spec §6 step 3:
///   - 1 if `last_click.entity != entity`
///   - 1 if `last_click.cell != cell`
///   - 1 if `now - last_click.at >= double_click_timeout`
///   - 1 if cursor drift exceeds `click_drift_px`
///   - else `(last_click.count % 3) + 1` (triple wraps to 1)
#[allow(dead_code)] // wired into dispatch_mouse_buttons in subsequent tasks
pub(crate) fn next_click_count(
    state: &mut MouseSelectionState,
    cfg: &ozmux_configs::mouse::MouseConfig,
    entity: Entity,
    cell: CellCoord,
    cursor_logical: Vec2,
    now: Instant,
) -> u8 {
    let timeout = std::time::Duration::from_millis(cfg.double_click_timeout_ms as u64);
    let drift_sq = cfg.click_drift_px * cfg.click_drift_px;
    let count = match state.last_click.as_ref() {
        Some(prev)
            if prev.entity == entity
                && prev.cell == cell
                && now.duration_since(prev.at) < timeout
                && (cursor_logical - prev.cursor_pos_logical_px).length_squared() <= drift_sq =>
        {
            (prev.count % 3) + 1
        }
        _ => 1,
    };
    state.last_click = Some(LastClick {
        entity,
        cell,
        cursor_pos_logical_px: cursor_logical,
        at: now,
        count,
    });
    count
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

    #[test]
    fn click_count_resets_on_first_press() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let count = super::next_click_count(
            &mut state,
            &cfg,
            Entity::from_bits(1),
            CellCoord { col: 5, row: 5 },
            Vec2::new(10.0, 10.0),
            now,
        );
        assert_eq!(count, 1);
    }

    #[test]
    fn click_count_increments_within_timeout_same_cell() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let e = Entity::from_bits(1);
        let cell = CellCoord { col: 5, row: 5 };
        let pos = Vec2::new(10.0, 10.0);
        let _ = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c2 = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c3 = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c4 = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        assert_eq!(c2, 2);
        assert_eq!(c3, 3);
        assert_eq!(c4, 1, "triple-click wraps back to 1");
    }

    #[test]
    fn click_count_resets_on_different_entity() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let cell = CellCoord { col: 5, row: 5 };
        let pos = Vec2::new(10.0, 10.0);
        let _ = super::next_click_count(&mut state, &cfg, Entity::from_bits(1), cell, pos, now);
        let c2 = super::next_click_count(&mut state, &cfg, Entity::from_bits(2), cell, pos, now);
        assert_eq!(c2, 1, "different entity must reset the counter");
    }

    #[test]
    fn click_count_resets_on_drift_beyond_threshold() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let e = Entity::from_bits(1);
        let cell = CellCoord { col: 5, row: 5 };
        let _ = super::next_click_count(&mut state, &cfg, e, cell, Vec2::new(10.0, 10.0), now);
        // Move 20 px away — exceeds the 8.0 default drift threshold.
        let c2 = super::next_click_count(&mut state, &cfg, e, cell, Vec2::new(40.0, 10.0), now);
        assert_eq!(c2, 1);
    }

    #[test]
    fn click_count_resets_after_timeout() {
        use std::time::Duration;
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let later = now + Duration::from_millis(500); // > 400ms default
        let e = Entity::from_bits(1);
        let cell = CellCoord { col: 5, row: 5 };
        let pos = Vec2::new(10.0, 10.0);
        let _ = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c2 = super::next_click_count(&mut state, &cfg, e, cell, pos, later);
        assert_eq!(c2, 1);
    }

    fn mock_cfg() -> ozmux_configs::mouse::MouseConfig {
        ozmux_configs::mouse::MouseConfig::default()
    }
}
