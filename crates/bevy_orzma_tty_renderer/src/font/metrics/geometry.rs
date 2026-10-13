//! The cell size and baseline measured from a face's metrics at one pixel
//! size.

use crate::font::Baseline;
use crate::font::metrics::font_units_to_px;
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
    /// baseline sits the rounded sum of the ascent and the line gap below
    /// the cell top, raised as far as the rounded descent needs to stay
    /// inside the cell, but never above the rounded ascent. The
    /// strike-through line is centered half a stroke below the face's
    /// strike-out position, or half the rounded ascent above the baseline
    /// when that center would not be above the baseline.
    ///
    /// `metrics.units_per_em` is not zero.
    pub fn measure(metrics: &Metrics, zero_advance: f32, px: f32) -> Self {
        let to_px = |units| font_units_to_px(units, metrics.units_per_em, px);
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
        let strike_height = to_px(metrics.strikeout_offset - metrics.stroke_size * 0.5);
        let baseline = if strike_height > 0.0 {
            baseline.with_strike_height(strike_height)
        } else {
            baseline
        };
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

    /// Consolas's hhea metrics, 2048 units per em, which carry a line gap,
    /// with its OS/2 strike-out position and `post` stroke.
    fn consolas() -> Metrics {
        Metrics {
            units_per_em: 2048,
            ascent: 1521.0,
            descent: 527.0,
            leading: 350.0,
            strikeout_offset: 512.0,
            stroke_size: 144.0,
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
            strikeout_offset: 320.0,
            stroke_size: 50.0,
            ..Metrics::default()
        }
    }

    /// Asserts the row height and the baseline a face with a line gap gets
    /// at the default sizes: the gap goes above the ascent while the
    /// rounded descent stays above the row bottom.
    ///
    /// Case: the user configures Consolas at the default 11.25 px size,
    /// which is 11 px at 100% scale and 17 px at 150%, or at 12 px.
    #[test]
    fn a_line_gap_lowers_the_baseline_at_the_default_sizes() {
        for (px, height, depth, gap_above) in [
            (11.0, 12.0, 9.0, 1.0),
            (12.0, 14.0, 11.0, 2.0),
            (17.0, 19.0, 15.0, 2.0),
        ] {
            let geometry = CellGeometry::measure(&consolas(), 1126.0, px);
            assert_eq!(geometry.cell_size().y, height, "{px} px");
            assert_eq!(*geometry.baseline(), depth, "{px} px");
            assert_eq!(geometry.baseline().gap_above(), gap_above, "{px} px");
        }
    }

    /// Asserts that, with a line gap, the baseline never cuts the rounded
    /// ascent and leaves the rounded descent above the row bottom at every
    /// size from 6 to 68 pixels.
    ///
    /// Case: the user steps a Consolas terminal at the default 11.25 px
    /// through every font size shortcut, on displays from 100% to 200%
    /// scale.
    #[test]
    fn a_line_gap_baseline_keeps_the_ascent_and_the_descent_at_every_size() {
        for px in 6..=68u16 {
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

    /// Asserts that the strike-through line is centered half a stroke below
    /// the face's strike-out position, measured from a baseline that the
    /// line gap lowered.
    ///
    /// Case: a program strikes through text drawn in Consolas at 12 px.
    #[test]
    fn the_strike_through_line_follows_the_face_strike_out_position() {
        let baseline = CellGeometry::measure(&consolas(), 1126.0, 12.0).baseline();
        assert_eq!(*baseline, 11.0);
        assert_eq!(
            baseline.strike_depth(),
            11.0 - (512.0 - 144.0 / 2.0) * 12.0 / 2048.0
        );
    }

    /// Asserts that a face with no strike-out position keeps the
    /// strike-through line half the rounded ascent above the baseline.
    ///
    /// Case: the user configures a font that has no OS/2 table, drawn at
    /// 12 px.
    #[test]
    fn a_face_without_a_strike_out_position_strikes_half_the_ascent_up() {
        let metrics = Metrics {
            strikeout_offset: 0.0,
            ..consolas()
        };
        let baseline = CellGeometry::measure(&metrics, 1126.0, 12.0).baseline();
        assert_eq!(baseline.strike_depth(), 11.0 - 9.0 / 2.0);
    }

    /// Asserts that a face without a line gap keeps the rounded ascent as
    /// its baseline at every size from 8 to 48 pixels.
    ///
    /// Case: the user keeps the bundled JetBrains Mono, whose line gap is
    /// zero.
    #[test]
    fn a_face_without_a_line_gap_keeps_the_ascent_baseline() {
        for px in 8..=48u16 {
            let px = f32::from(px);
            let baseline = CellGeometry::measure(&jetbrains_mono(), 600.0, px).baseline();
            assert_eq!(*baseline, (1020.0 * px / 1000.0).round(), "{px} px");
            assert_eq!(baseline.gap_above(), 0.0, "{px} px");
        }
    }
}
