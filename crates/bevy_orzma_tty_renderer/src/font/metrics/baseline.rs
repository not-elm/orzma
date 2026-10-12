//! The baseline and the strike-through line of a row of cells.

use bevy::prelude::Deref;

/// The baseline's depth below the top of a cell, in whole physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Deref)]
pub struct Baseline {
    /// A ascent physical pixels.
    #[deref]
    depth: f32,
    /// `depth` * 0.5
    strike_depth: f32,
}

impl Baseline {
    /// Creates the baseline from a face's ascent of `ascent` physical pixels.
    ///
    /// The ascent is rounded to the nearest whole pixel, and the
    /// strike-through line sits halfway down to the baseline.
    pub fn new(ascent: f32) -> Self {
        let depth = ascent.round();
        Self {
            depth,
            strike_depth: depth * 0.5,
        }
    }

    /// Creates the baseline with line-gap.
    pub fn with_line_gap(ascent: f32, descent: f32, line_gap: f32, line_height: f32) -> Self {
        let raised = ascent.round();
        let depth = raised.max(
            (ascent + line_gap)
                .round()
                .min(line_height - descent.round()),
        );
        Self {
            depth,
            strike_depth: depth - raised * 0.5,
        }
    }

    /// The depth of the strike-through line's center below the top of a cell, in physical pixels.
    pub fn strike_depth(&self) -> f32 {
        self.strike_depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a baseline built from an ascent is the rounded ascent,
    /// with the strike-through line halfway down to it.
    ///
    /// Case: a host builds cell metrics by hand from a face's ascent.
    #[test]
    fn a_baseline_from_the_ascent_strikes_through_half_of_it() {
        let baseline = Baseline::new(11.6);
        assert_eq!(*baseline, 12.0);
        assert_eq!(baseline.strike_depth(), 6.0);
    }
}
