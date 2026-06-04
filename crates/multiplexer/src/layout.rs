//! Structural types and arithmetic for the entity-based layout tree.
//! Currently owns `SplitOrientation`, `Side`, `Rect`, `split_ratio`, and the
//! `normalized_grows` invariant. The read-only `LayoutTree` query view,
//! spawn-bundle helpers, and tree mutations (split / close / swap) are added
//! incrementally across the rearch plan.

/// Split axis.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum SplitOrientation {
    /// Left and right children share horizontal space.
    Horizontal,
    /// Top and bottom children share vertical space.
    Vertical,
}

/// Which side of an existing cell a newly-inserted sibling lands on.
#[derive(Debug, Default, Clone, Copy, Hash, Eq, PartialEq)]
pub enum Side {
    /// Place the new node before the target (left or top).
    Before,
    /// Place the new node after the target (right or bottom).
    #[default]
    After,
}

/// Axis-aligned rectangle in normalized workspace coordinates (`x, y, w, h` ∈ [0, 1]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge in [0, 1].
    pub x: f32,
    /// Top edge in [0, 1].
    pub y: f32,
    /// Width in [0, 1].
    pub w: f32,
    /// Height in [0, 1].
    pub h: f32,
}

/// Normalize a split's two child weights so they are never *both* zero
/// (which would flex-collapse the subtree to zero size). `(0.0, 0.0)` →
/// `(1.0, 1.0)`; any pair with at least one nonzero passes through. This is
/// the single chokepoint for the never-both-zero invariant; all split-child
/// `flex_grow` writes go through `set_split_grows`, which calls this.
pub(crate) fn normalized_grows(lhs: f32, rhs: f32) -> (f32, f32) {
    if lhs == 0.0 && rhs == 0.0 {
        (1.0, 1.0)
    } else {
        (lhs, rhs)
    }
}

/// Normalize a `(lhs_weight, rhs_weight)` pair to a `[0, 1]` ratio (the lhs
/// fraction). Returns `0.5` when both are zero. Mirrors the old
/// `LayoutCellState::split_ratio` so the resize math ports unchanged.
pub fn split_ratio(lhs_weight: f32, rhs_weight: f32) -> f32 {
    let total = lhs_weight + rhs_weight;
    if total == 0.0 { 0.5 } else { lhs_weight / total }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_split_grows_clamps_double_zero_to_one_one() {
        assert_eq!(normalized_grows(0.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn set_split_grows_passes_through_nonzero() {
        assert_eq!(normalized_grows(0.25, 0.75), (0.25, 0.75));
        assert_eq!(normalized_grows(3.0, 0.0), (3.0, 0.0));
    }
}
