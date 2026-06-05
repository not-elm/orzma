//! Layout geometry: normalized rectangles, integer cell resolution, and
//! the resize ratio math — ported from the (already pure) `multiplexer`
//! arithmetic onto the `ozmux_mux` arena.

/// Axis-aligned rectangle in normalized `[0,1]` workspace coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// The first-child fraction from a `(lhs, rhs)` weight pair; `0.5` when both
/// are zero. Parity with the old `split_ratio`.
pub fn split_ratio(lhs: f32, rhs: f32) -> f32 {
    let total = lhs + rhs;
    if total == 0.0 { 0.5 } else { lhs / total }
}

/// Integer cell distribution of `p` cells by `ratio` (first child's
/// fraction). `lhs = round_ties_even(p * ratio)` capped at `p`, `rhs = p - lhs`
/// so the two ALWAYS sum to `p` (no rounding drift). Port of `resize.rs::split_cells`.
pub fn split_cells(p: u16, ratio: f32) -> (u16, u16) {
    let lhs = ((f32::from(p) * ratio).round_ties_even() as u16).min(p);
    (lhs, p - lhs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_cells_rounds_ties_even_and_caps_at_p() {
        // split_ratio(1.0, 1.0) = 0.5 → (60, 60)
        assert_eq!(split_cells(120, split_ratio(1.0, 1.0)), (60, 60));
        // split_ratio(0.0, 0.0) = 0.5 → (60, 60) (zero-total rescue)
        assert_eq!(split_cells(120, split_ratio(0.0, 0.0)), (60, 60));
        // split_ratio(110.0, 10.0) = 110/120 → (110, 10)
        assert_eq!(split_cells(120, split_ratio(110.0, 10.0)), (110, 10));
    }

    #[test]
    fn split_ratio_handles_zero_total() {
        assert_eq!(split_ratio(0.0, 0.0), 0.5);
        assert_eq!(split_ratio(3.0, 1.0), 0.75);
    }
}
