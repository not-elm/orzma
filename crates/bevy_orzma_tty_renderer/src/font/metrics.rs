//! The cell metrics the grid lays out with, and the resource that shares
//! them.

use crate::font::TerminalFonts;
use bevy::prelude::{Resource, Vec2};

/// Pixel metrics for the regular face at the given physical pixel size.
#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    /// Horizontal advance of glyph `'0'` in physical pixels.
    pub advance_phys: f32,
    /// Ascent + |descent| + line_gap in physical pixels.
    pub line_height_phys: f32,
    /// Distance from baseline to top of em-box in physical pixels (positive).
    pub ascent_phys: f32,
    /// Distance from baseline to bottom of em-box in physical pixels (positive).
    pub descent_phys: f32,
    /// Offset from baseline to underline-stroke CENTER in physical pixels.
    /// Negative because the underline sits below the baseline. (OpenType
    /// `post.underlinePosition` convention.)
    pub underline_position_phys: f32,
    /// Underline stroke thickness in physical pixels.
    pub underline_thickness_phys: f32,
    /// Worst-case rightward overflow in physical px across all four faces
    /// (Regular/Italic/Bold/BoldItalic) over ASCII printable codepoints,
    /// measured as `max(0, outline_glyph(...).px_bounds().max.x - cell_w_phys_floor)`.
    /// A host laying out a terminal node must reserve this much width past
    /// the grid rectangle.
    pub max_overflow_phys: f32,
}

impl CellMetrics {
    /// The cell pitch the grid lays out at, in physical pixels: the advance
    /// and the line height, each floored and at least one pixel.
    pub fn cell_size_phys(&self) -> Vec2 {
        Vec2::new(
            self.advance_phys.floor().max(1.0),
            self.line_height_phys.floor().max(1.0),
        )
    }
}

/// The canonical cell pitch and advance values.
///
/// It is inserted at startup from the PrimaryWindow's scale_factor, and
/// rewritten, with the change marked, only when the physical font size —
/// the font size times the primary window's scale factor, rounded —
/// differs from the one `metrics` was measured at. A scale factor change
/// that rounds to the same physical size leaves it untouched. It already
/// reflects the primary window's current scale factor when `Update` runs.
#[derive(Resource, Clone, Copy, Debug)]
pub struct TerminalCellMetricsResource {
    /// Current cell pitch and typographic measurements in physical pixels.
    pub metrics: CellMetrics,
    /// Physical font size (in pixels) that `metrics` was computed at.
    pub phys_font_size: u16,
}

impl TerminalCellMetricsResource {
    /// The metrics of `fonts` measured at `phys_font_size` physical pixels.
    pub fn new(fonts: &TerminalFonts, phys_font_size: u16) -> Self {
        Self {
            metrics: fonts.cell_metrics_px(phys_font_size),
            phys_font_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the cell pitch floors each axis and never drops below
    /// one pixel.
    ///
    /// Case: a fractional font size measures a 7.6 by 15.4 pixel cell, and
    /// a degenerate face measures a zero-width advance.
    #[test]
    fn cell_size_phys_floors_each_axis_to_at_least_one_pixel() {
        let metrics = |advance_phys, line_height_phys| CellMetrics {
            advance_phys,
            line_height_phys,
            ascent_phys: 0.0,
            descent_phys: 0.0,
            underline_position_phys: 0.0,
            underline_thickness_phys: 0.0,
            max_overflow_phys: 0.0,
        };
        assert_eq!(metrics(7.6, 15.4).cell_size_phys(), Vec2::new(7.0, 15.0));
        assert_eq!(metrics(0.0, 0.4).cell_size_phys(), Vec2::new(1.0, 1.0));
    }
}
