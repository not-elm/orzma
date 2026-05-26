//! IME preedit overlay — pure pixel-math layer.
//!
//! Provides `compute_overlay_pos`, the single source of truth for the
//! inline preedit overlay's logical-pixel position. The Bevy plugin
//! (`ImeOverlayPlugin`) and its `position_ime_overlay` system are
//! added in later tasks; this commit only ships the pure function and
//! its unit tests.

use bevy::math::Vec2;
use bevy_terminal_renderer::CellMetrics;

/// Computes the overlay's top-left logical-pixel position relative to
/// the window origin. Caller is responsible for writing this into
/// `Node.left` / `Node.top`.
///
/// All metric inputs are physical px; the function does the
/// physical→logical conversion via `scale`.
///
/// Layout: the overlay sits **one row below** the cursor cell so the
/// inline preedit doesn't overlap with the active-line glyph still
/// rendered by the terminal material. Clamps:
///   - right: if `cell_origin_x + measured_width > host_right`,
///     shifts left so the right edge stays inside the host rect.
///   - left: after the right-edge clamp, ensures `left >= host_left`
///     so a very wide composition can't escape the left side of the
///     pane.
pub(crate) fn compute_overlay_pos(
    ui_global_translation: Vec2,
    host_size_logical: Vec2,
    cursor_cell: (u16, u16),
    metrics: &CellMetrics,
    measured_width_logical: f32,
    scale: f32,
) -> Vec2 {
    let cell_w_phys = metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.line_height_phys.floor().max(1.0);
    let host_origin_phys = ui_global_translation * scale;
    let cell_origin_phys = host_origin_phys
        + Vec2::new(
            cursor_cell.0 as f32 * cell_w_phys,
            (cursor_cell.1 as f32 + 1.0) * cell_h_phys,
        );
    let pos_logical = cell_origin_phys / scale;

    let host_right = ui_global_translation.x + host_size_logical.x;
    let mut left = pos_logical.x;
    if left + measured_width_logical > host_right {
        left = host_right - measured_width_logical;
    }
    let host_left = ui_global_translation.x;
    if left < host_left {
        left = host_left;
    }

    Vec2::new(left, pos_logical.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `CellMetrics` literal for tests. `CellMetrics` does not
    /// derive `Default`, so callers must provide every field; this
    /// helper takes the two that `compute_overlay_pos` actually reads
    /// (`advance_phys`, `line_height_phys`) and fills the rest with
    /// arbitrary non-zero values that don't affect the function under
    /// test.
    fn metrics(advance: f32, line_height: f32) -> CellMetrics {
        CellMetrics {
            advance_phys: advance,
            line_height_phys: line_height,
            ascent_phys: 12.0,
            descent_phys: 4.0,
            underline_position_phys: -2.0,
            underline_thickness_phys: 1.0,
            max_overflow_phys: 0.0,
        }
    }

    #[test]
    fn places_overlay_one_row_below_cursor() {
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
            (3, 5),
            &metrics(10.0, 16.0),
            0.0,
            1.0,
        );
        // y = (row 5 + 1) × 16 = 96
        assert_eq!(pos.y, 96.0);
        // x = col 3 × 10 = 30, no clamp
        assert_eq!(pos.x, 30.0);
    }

    #[test]
    fn divides_by_scale_factor_for_logical_px() {
        // translation (100, 0) logical at scale 2.0 → host_origin_phys (200, 0)
        // cell (0, 0) row-below → cell_origin_phys (200, 16) → logical (100, 8)
        let pos = compute_overlay_pos(
            Vec2::new(100.0, 0.0),
            Vec2::new(800.0, 600.0),
            (0, 0),
            &metrics(10.0, 16.0),
            0.0,
            2.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 8.0);
    }

    #[test]
    fn floors_subpixel_cell_pitch() {
        // advance 10.4 → floor 10; col 10 → x = 100
        // line_height 16.4 → floor 16; row 1 row-below → y = (1+1) × 16 = 32
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
            (10, 1),
            &metrics(10.4, 16.4),
            0.0,
            1.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 32.0);
    }

    #[test]
    fn clamps_right_when_overlay_overflows() {
        // Cursor at col 78, cell width 10 → cell_origin x = 780.
        // Measured width 100 → would extend to 880, host right = 800.
        // Shift left by 80 → left = 700.
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
            (78, 0),
            &metrics(10.0, 16.0),
            100.0,
            1.0,
        );
        assert_eq!(pos.x, 700.0);
    }

    #[test]
    fn clamps_left_when_composition_too_wide_to_fit() {
        // host_size 80 (very narrow), measured 200, cursor at col 7 →
        // cell_origin x = 70, would overflow right → shift to
        // host_right - measured = 80 - 200 = -120, then left clamp →
        // 0 (host_left).
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(80.0, 600.0),
            (7, 0),
            &metrics(10.0, 16.0),
            200.0,
            1.0,
        );
        assert_eq!(pos.x, 0.0);
    }
}
