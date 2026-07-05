//! Shared terminal-surface geometry: cursor physical-pixel → pane-local px →
//! cell (column/row/side). Mode-agnostic — used by the pointer router
//! (`crate::input::mouse::webview`), both mode pipelines, and hyperlink hover.
//! The `TmuxPane`-specific hit-test lives in `crate::input::tmux::pane_hit`.

use bevy::ecs::entity::Entity;
use bevy::math::Vec2;
use bevy::ui::{ComputedNode, UiGlobalTransform};

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

/// Divides `v` by `pitch`, clamped to non-negative, and splits the result into
/// its floored cell index and the fractional (sub-cell) remainder.
fn floor_frac(v: f32, pitch: f32) -> (u32, f32) {
    let q = (v / pitch).max(0.0);
    let floor = q.floor();
    (floor as u32, q - floor)
}

/// Which half of a cell `frac_x` (the fractional part of a column coordinate)
/// falls in.
fn side_of(frac_x: f32) -> Side {
    if frac_x < 0.5 {
        Side::Left
    } else {
        Side::Right
    }
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
    let (col_floor, frac_x) = floor_frac(local_phys.x, cell_w_phys);
    let (row_floor, _) = floor_frac(local_phys.y, cell_h_phys);
    let col = (col_floor + 1).min(cols as u32).max(1);
    let row = (row_floor + 1).min(rows as u32).max(1);
    (col, row, side_of(frac_x))
}

/// Maps a window cursor position (physical px) to the active `TmuxPane`'s
/// visible `(col, row, side)`, clamped to `[0, cols) × [0, rows)`. Returns
/// `None` when the projection is degenerate (zero-area node). The point is
/// clamped (not rejected) when it falls outside the pane so a drag that leaves
/// the pane edge still extends the selection to the nearest cell.
pub(crate) fn cell_at_pane(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16, Side)> {
    let local = phys_to_pane_local(node, transform, cursor_phys)?;
    let (col_floor, frac_x) = floor_frac(local.x, cell_w_phys);
    let (row_floor, _) = floor_frac(local.y, cell_h_phys);
    let col = col_floor.min(cols.saturating_sub(1) as u32);
    let row = row_floor.min(rows.saturating_sub(1) as u32);
    Some((col as u16, row as u16, side_of(frac_x)))
}

/// Computes terminal dimensions in cells from physical pixel size.
///
/// Returns `(cols, rows)`, each clamped to a minimum of 1.
pub(crate) fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

/// Returns the topmost `OrzmaTerminal` surface whose node contains `cursor_phys`,
/// or `None` when the cursor is over none. "Topmost" is the highest
/// `ComputedNode::stack_index` (Bevy's resolved front-to-back UI order); ties
/// break by `Entity` for determinism. The Default-mode pointer/gate path uses
/// this to pick the single shell (or the frontmost surface) under the cursor;
/// tmux keeps its own multi-pane `tmux_pane_at_phys` resolution.
pub(crate) fn topmost_surface_at<'a>(
    cursor_phys: Vec2,
    candidates: impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)>,
) -> Option<Entity> {
    candidates
        .filter(|&(_, node, transform)| node.contains_point(*transform, cursor_phys))
        .max_by_key(|&(entity, node, _)| (node.stack_index(), entity))
        .map(|(entity, _, _)| entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use bevy::math::Vec2;

    #[test]
    fn cell_at_local_is_one_indexed_and_clamped() {
        let (col, row, side) = cell_at_local(Vec2::new(0.0, 0.0), 10.0, 20.0, 80, 24);
        assert_eq!(
            (col, row),
            (1, 1),
            "origin maps to the 1-indexed top-left cell"
        );
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

    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(1, 1, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }

    #[test]
    fn cell_at_pane_maps_and_clamps() {
        // A point at local (40, 48) with 8x16 px cells maps to col 5, row 3
        // (floor(40/8)=5, floor(48/16)=3); cols/rows bound the clamp, not the node.
        let node = ComputedNode {
            size: Vec2::new(640.0, 384.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(320.0, 192.0);
        let cell = cell_at_pane(&node, &transform, Vec2::new(40.0, 48.0), 8.0, 16.0, 80, 24);
        assert_eq!(cell, Some((5, 3, Side::Left)));
    }

    #[test]
    fn cell_at_pane_clamps_past_the_far_edge() {
        let node = ComputedNode {
            size: Vec2::new(640.0, 384.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(320.0, 192.0);
        // A point well past the bottom-right clamps to (cols-1, rows-1).
        let cell = cell_at_pane(
            &node,
            &transform,
            Vec2::new(9999.0, 9999.0),
            8.0,
            16.0,
            80,
            24,
        );
        assert_eq!(cell, Some((79, 23, Side::Right)));
    }

    #[test]
    fn cell_at_pane_clamps_negative_to_origin() {
        let node = ComputedNode {
            size: Vec2::new(640.0, 384.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(320.0, 192.0);
        // A point above-left of the node clamps to (0, 0).
        let cell = cell_at_pane(
            &node,
            &transform,
            Vec2::new(-50.0, -50.0),
            8.0,
            16.0,
            80,
            24,
        );
        assert_eq!(cell, Some((0, 0, Side::Left)));
    }

    #[test]
    fn topmost_surface_at_picks_highest_stack_index_among_containing() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let c = world.spawn_empty().id();
        // A: left half (x 0..400), stack 5. B: right half (x 400..800), stack 3.
        // C: left half, stack 9 — overlaps A and sits on top.
        let node_a = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 5,
            ..ComputedNode::DEFAULT
        };
        let tf_a = UiGlobalTransform::from_xy(200.0, 300.0);
        let node_b = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 3,
            ..ComputedNode::DEFAULT
        };
        let tf_b = UiGlobalTransform::from_xy(600.0, 300.0);
        let node_c = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 9,
            ..ComputedNode::DEFAULT
        };
        let tf_c = UiGlobalTransform::from_xy(200.0, 300.0);
        let candidates = [
            (a, &node_a, &tf_a),
            (b, &node_b, &tf_b),
            (c, &node_c, &tf_c),
        ];

        assert_eq!(
            topmost_surface_at(Vec2::new(600.0, 300.0), candidates.iter().copied()),
            Some(b),
            "a point only B contains must resolve to B"
        );
        assert_eq!(
            topmost_surface_at(Vec2::new(100.0, 300.0), candidates.iter().copied()),
            Some(c),
            "where A and C overlap, the higher stack_index (C) wins"
        );
        assert_eq!(
            topmost_surface_at(Vec2::new(2000.0, 2000.0), candidates.iter().copied()),
            None,
            "a point outside every node resolves to None"
        );
    }

    #[test]
    fn topmost_surface_at_breaks_stack_index_ties_deterministically() {
        let mut world = World::new();
        let lower = world.spawn_empty().id();
        let higher = world.spawn_empty().id();
        // Two fully-overlapping nodes with the SAME stack_index (only reachable
        // before the first layout pass assigns indices). The winner must not
        // depend on candidate iteration order.
        let node = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 0,
            ..ComputedNode::DEFAULT
        };
        let tf = UiGlobalTransform::from_xy(200.0, 300.0);
        let forward = [(lower, &node, &tf), (higher, &node, &tf)];
        let reversed = [(higher, &node, &tf), (lower, &node, &tf)];
        let winner = topmost_surface_at(Vec2::new(100.0, 300.0), forward.iter().copied());
        assert_eq!(
            winner,
            topmost_surface_at(Vec2::new(100.0, 300.0), reversed.iter().copied()),
            "tie resolution must not depend on iteration order"
        );
        assert_eq!(
            winner,
            Some(lower.max(higher)),
            "a stack_index tie resolves by Entity order, deterministically"
        );
    }
}
