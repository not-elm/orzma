//! The cell metrics the grid lays out with, and the resource that shares
//! them.

use crate::font::TerminalFonts;
use bevy::prelude::{Resource, Vec2};

/// The cell the grid lays out with, measured from the regular face at one
/// physical pixel size and snapped to the whole pixels the cells sit on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// The cell pitch in physical pixels: the advance of glyph `'0'` and
    /// the line height (ascent + |descent| + line gap), each floored and at
    /// least one pixel.
    pub cell_size: Vec2,
    /// The baseline's depth below the top of a cell, in whole physical pixels:
    /// the ascent rounded to the nearest pixel.
    pub baseline: f32,
    /// The underline stroke.
    pub underline: Underline,
    /// Worst-case rightward overflow in physical px across all four faces
    /// (Regular/Italic/Bold/BoldItalic) over ASCII printable codepoints: the
    /// furthest an outline's right edge, rounded up to a whole pixel,
    /// reaches past the cell width, or 0. A host laying out a terminal node
    /// must reserve this much width past the grid rectangle.
    pub max_overflow: f32,
}

/// A horizontal stroke across a cell, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Underline {
    /// Physical offset from the baseline to the stroke's TOP edge.
    /// Negative below the baseline.
    pub position: f32,
    /// Physical stroke thickness, at least one pixel.
    pub thickness: f32,
}

/// The canonical cell metrics.
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
