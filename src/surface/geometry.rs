//! Shared terminal-surface geometry: cursor physical-pixel → pane-local px →
//! cell (column/row/side). Mode-agnostic — used by the pointer router
//! (`crate::webview_pointer`), both mode pipelines, and hyperlink hover. The
//! `TmuxPane`-specific hit-test lives in `crate::input::tmux::pane_hit`.

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

/// Maps a window cursor position (physical px) to the active `TmuxPane`'s
/// visible `(col, row)`, clamped to `[0, cols) × [0, rows)`. Returns `None` when
/// the projection is degenerate (zero-area node). The point is clamped (not
/// rejected) when it falls outside the pane so a drag that leaves the pane edge
/// still extends the selection to the nearest cell.
pub(crate) fn cell_at_pane(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    let local = phys_to_pane_local(node, transform, cursor_phys)?;
    let col = ((local.x / cell_w_phys).floor().max(0.0) as u32).min(cols.saturating_sub(1) as u32);
    let row = ((local.y / cell_h_phys).floor().max(0.0) as u32).min(rows.saturating_sub(1) as u32);
    Some((col as u16, row as u16))
}

/// Computes terminal dimensions in cells from physical pixel size.
///
/// Returns `(cols, rows)`, each clamped to a minimum of 1.
pub(crate) fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(cell, Some((5, 3)));
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
        assert_eq!(cell, Some((79, 23)));
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
        assert_eq!(cell, Some((0, 0)));
    }
}
