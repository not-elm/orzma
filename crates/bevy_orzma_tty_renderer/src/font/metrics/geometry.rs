//! The cell size and baseline measured from a face's metrics at one pixel
//! size.

use crate::font::Baseline;
use bevy::math::Vec2;
use swash::Metrics;

/// A cell's size and baseline at one physical pixel size, measured from a
/// face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellGeometry {
    cell_size: Vec2,
    baseline: Baseline,
}

impl CellGeometry {
    /// Measures the cell of a face with `metrics`, whose `'0'` glyph is
    /// `zero_advance` font units wide, at `px` physical pixels per em.
    ///
    /// The cell is the advance of `'0'` by the line height (ascent +
    /// descent + line gap), each floored and at least one pixel. The
    /// baseline follows [`Baseline::with_line_gap`].
    pub fn measure(metrics: &Metrics, zero_advance: f32, px: f32) -> Self {
        let upem = f32::from(metrics.units_per_em);
        let to_px = |units: f32| units * px / upem;
        let cell_size = Vec2::new(
            to_px(zero_advance),
            to_px(metrics.ascent + metrics.descent + metrics.leading),
        )
        .floor()
        .max(Vec2::ONE);
        let baseline = Baseline::with_line_gap(
            to_px(metrics.ascent),
            to_px(metrics.descent),
            to_px(metrics.leading),
            cell_size.y,
        );
        Self {
            cell_size,
            baseline,
        }
    }

    /// The cell pitch in physical pixels.
    pub fn cell_size(&self) -> Vec2 {
        self.cell_size
    }

    /// The baseline of a row of these cells.
    pub fn baseline(&self) -> Baseline {
        self.baseline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swash::Metrics;

    /// Consolas's hhea metrics, 2048 units per em, which carry a line gap.
    fn consolas() -> Metrics {
        Metrics {
            units_per_em: 2048,
            ascent: 1521.0,
            descent: 527.0,
            leading: 350.0,
            ..Metrics::default()
        }
    }

    /// JetBrains Mono's metrics, 1000 units per em, with no line gap.
    fn jetbrains_mono() -> Metrics {
        Metrics {
            units_per_em: 1000,
            ascent: 1020.0,
            descent: 300.0,
            leading: 0.0,
            ..Metrics::default()
        }
    }

    /// Asserts the row height and the baseline a face with a line gap gets
    /// at the default sizes: the gap goes above the ascent while the
    /// rounded descent stays above the row bottom.
    ///
    /// Case: orzma draws with Windows's default Consolas at the default
    /// 11.25 px, which is 11 px at 100% scale and 17 px at 150%, and at
    /// 12 px.
    #[test]
    fn a_line_gap_lowers_the_baseline_at_the_default_sizes() {
        for (px, height, depth) in [(11.0, 12.0, 9.0), (12.0, 14.0, 11.0), (17.0, 19.0, 15.0)] {
            let geometry = CellGeometry::measure(&consolas(), 1126.0, px);
            assert_eq!(geometry.cell_size().y, height, "{px} px");
            assert_eq!(*geometry.baseline(), depth, "{px} px");
        }
    }

    /// Asserts that, with a line gap, the baseline never cuts the rounded
    /// ascent and leaves the rounded descent above the row bottom at every
    /// size from 9 to 32 pixels.
    ///
    /// Case: the user zooms a Consolas terminal through the sizes the font
    /// size shortcuts reach.
    #[test]
    fn a_line_gap_baseline_keeps_the_ascent_and_the_descent_at_every_size() {
        for px in 9..=32u16 {
            let px = f32::from(px);
            let geometry = CellGeometry::measure(&consolas(), 1126.0, px);
            let ascent = (1521.0 * px / 2048.0_f32).round();
            let descent = (527.0 * px / 2048.0_f32).round();
            let depth = *geometry.baseline();
            assert!(depth >= ascent, "{px} px cuts the ascent");
            assert!(
                geometry.cell_size().y - depth >= descent,
                "{px} px cuts the descent"
            );
        }
    }

    /// Asserts that the strike-through line sits half the rounded ascent
    /// above a baseline that the line gap lowered.
    ///
    /// Case: a program strikes through text drawn in Consolas at 12 px.
    #[test]
    fn a_lowered_baseline_keeps_the_strike_through_half_the_ascent_above_it() {
        let baseline = CellGeometry::measure(&consolas(), 1126.0, 12.0).baseline();
        assert_eq!(*baseline, 11.0);
        assert_eq!(baseline.strike_depth(), 11.0 - 9.0 / 2.0);
    }

    /// Asserts that a face without a line gap gets the baseline and the
    /// strike-through line that `Baseline::new` gives for its ascent, at
    /// every size from 8 to 48 pixels.
    ///
    /// Case: the user configures JetBrains Mono, whose line gap is zero.
    #[test]
    fn a_face_without_a_line_gap_keeps_the_ascent_baseline() {
        for px in 8..=48u16 {
            let px = f32::from(px);
            let geometry = CellGeometry::measure(&jetbrains_mono(), 600.0, px);
            assert_eq!(
                geometry.baseline(),
                Baseline::new(1020.0 * px / 1000.0),
                "{px} px"
            );
        }
    }
}
