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
}
