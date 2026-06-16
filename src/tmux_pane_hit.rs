//! Shared tmux pane hit-testing and cell geometry. One home for the pointer
//! → (pane, local-physical-px, cell) math used by hyperlink hover, the mouse
//! arbiter, inline-webview pointer routing, and copy-mode.

use bevy::ecs::entity::Entity;
use bevy::ecs::system::Query;
use bevy::math::Vec2;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use ozmux_tmux::{PaneId, TmuxPane};

/// Which half of a cell the pointer fell in (left vs. right of the midline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

/// Pointer in pane-local physical px (origin = pane node top-left), or `None`
/// if outside the node.
pub(crate) fn phys_to_pane_local(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys_px: Vec2,
) -> Option<Vec2> {
    node.normalize_point(*transform, cursor_phys_px)
        .map(|normalized| (normalized + Vec2::splat(0.5)) * node.size)
}

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

/// 1-indexed `(col, row, side)` of the cell at `local_phys`, clamped to the
/// grid. Clamps `col` to `1..=cols` and `row` to `1..=rows`. `cell_w_phys` /
/// `cell_h_phys` are the physical-pixel cell pitch from
/// `TerminalCellMetricsResource`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec2;

    #[test]
    fn cell_at_local_is_one_indexed_and_clamped() {
        let (col, row, side) = cell_at_local(Vec2::new(0.0, 0.0), 10.0, 20.0, 80, 24);
        assert_eq!((col, row), (1, 1), "origin maps to the 1-indexed top-left cell");
        assert_eq!(side, Side::Left, "the left edge of a cell is the Left side");

        let (col, row, _) = cell_at_local(Vec2::new(10_000.0, 10_000.0), 10.0, 20.0, 80, 24);
        assert_eq!(
            (col, row),
            (80, 24),
            "a point far past the grid clamps to the bottom-right cell"
        );

        let (col, _, side) = cell_at_local(Vec2::new(17.0, 5.0), 10.0, 20.0, 80, 24);
        assert_eq!(col, 2, "x=17 with a 10px pitch lands in the second column");
        assert_eq!(
            side,
            Side::Right,
            "x=17 is 0.7 into its cell, past the midline → Right"
        );
    }
}
