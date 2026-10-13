//! The baseline and the strike-through line of a row of cells.

use bevy::prelude::Deref;

/// The baseline's depth below the top of a cell, in whole physical pixels,
/// and the strike-through line placed from it.
#[derive(Clone, Copy, Debug, PartialEq, Deref)]
pub struct Baseline {
    /// The baseline's depth below the cell top.
    #[deref]
    depth: f32,
    /// The whole pixels of line gap between the rounded ascent and the baseline.
    gap_above: f32,
    /// The depth of the strike-through line's center below the cell top, in physical pixels.
    strike_depth: f32,
}

impl Baseline {
    /// Creates the baseline from a face's ascent of `ascent` physical pixels.
    ///
    /// The ascent is rounded to the nearest whole pixel and no line gap sits
    /// above it. The strike-through line sits halfway down to the baseline.
    pub fn new(ascent: f32) -> Self {
        let depth = ascent.round();
        Self {
            depth,
            gap_above: 0.0,
            strike_depth: depth * 0.5,
        }
    }

    /// Creates the baseline of a row `line_height` physical pixels tall for a
    /// face whose ascent, descent and line gap measure `ascent`, `descent` and
    /// `line_gap` physical pixels.
    ///
    /// The baseline sits the rounded sum of the ascent and the line gap below
    /// the row top, raised as far as the rounded descent needs to stay inside
    /// the row, but never above the rounded ascent. The strike-through line
    /// sits half the rounded ascent above the baseline.
    pub fn with_line_gap(ascent: f32, descent: f32, line_gap: f32, line_height: f32) -> Self {
        let raised = ascent.round();
        let depth = raised.max(
            (ascent + line_gap)
                .round()
                .min(line_height - descent.round()),
        );
        Self {
            depth,
            gap_above: depth - raised,
            strike_depth: depth - raised * 0.5,
        }
    }

    /// Moves the strike-through line's center to `height` physical pixels
    /// above the baseline.
    pub fn with_strike_height(self, height: f32) -> Self {
        Self {
            strike_depth: self.depth - height,
            ..self
        }
    }

    /// The whole physical pixels the line gap lowers the baseline below the
    /// rounded ascent, or 0 when it does not lower it.
    pub fn gap_above(&self) -> f32 {
        self.gap_above
    }

    /// The depth of the strike-through line's center below the top of a
    /// cell, in physical pixels.
    pub fn strike_depth(&self) -> f32 {
        self.strike_depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a baseline built from an ascent is the rounded ascent,
    /// with no line gap above it and the strike-through line halfway down
    /// to it.
    ///
    /// Case: a host builds cell metrics by hand from a face's ascent.
    #[test]
    fn a_baseline_from_the_ascent_strikes_through_half_of_it() {
        let baseline = Baseline::new(11.6);
        assert_eq!(*baseline, 12.0);
        assert_eq!(baseline.gap_above(), 0.0);
        assert_eq!(baseline.strike_depth(), 6.0);
    }

    /// Asserts that a strike height moves only the strike-through line,
    /// measured up from the baseline.
    ///
    /// Case: a face's strike-out position puts the line's center 2.5 pixels
    /// above a 12 pixel baseline.
    #[test]
    fn a_strike_height_moves_only_the_strike_through_line() {
        let baseline = Baseline::new(12.0).with_strike_height(2.5);
        assert_eq!(*baseline, 12.0);
        assert_eq!(baseline.gap_above(), 0.0);
        assert_eq!(baseline.strike_depth(), 9.5);
    }
}
