//! The DECSTBM scroll region and the DECOM cursor origin.

use crate::schema::ScreenLine;
use std::ops::RangeInclusive;

/// The scrolling region paired with the cursor origin it may impose.
///
/// DECSTBM and DECOM are orthogonal: the margins bound scrolling
/// whichever way the origin mode is set, and the origin mode only
/// decides whether the cursor is homed and clamped to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRegion {
    margins: Margins,
    origin_mode: OriginMode,
}

impl ScrollRegion {
    /// Builds the default region for a grid of `rows` rows: the whole
    /// page, with the cursor origin at the upper-left corner.
    pub fn new(rows: u16) -> Self {
        Self {
            margins: Margins::new(rows),
            origin_mode: OriginMode::default(),
        }
    }

    /// Returns the top margin.
    pub fn top_margin(&self) -> ScreenLine {
        self.margins.top
    }

    /// Returns the bottom margin.
    pub fn bottom_margin(&self) -> ScreenLine {
        self.margins.bottom
    }

    /// The rows a scroll moves: the top margin through the bottom
    /// margin, inclusive.
    ///
    /// The span never consults [`OriginMode`], because DECSTBM confines
    /// scrolling to the margins whichever way the origin is set.
    pub fn scroll_span(&self) -> RangeInclusive<ScreenLine> {
        self.margins.top..=self.margins.bottom
    }

    /// Moves the top margin, leaving the bottom margin and the origin
    /// mode alone.
    ///
    /// This exists for tests that need a region narrower than the page
    /// while DECSTBM is still unwired; the real control function will
    /// set both margins together.
    #[cfg(test)]
    pub(crate) fn set_top_margin(&mut self, top: ScreenLine) {
        self.margins.top = top;
    }

    /// Moves the bottom margin, leaving the top margin and the origin
    /// mode alone.
    ///
    /// This is the counterpart of [`Self::set_top_margin`] and carries
    /// the same caveat.
    #[cfg(test)]
    pub(crate) fn set_bottom_margin(&mut self, bottom: ScreenLine) {
        self.margins.bottom = bottom;
    }
}

/// Whether the cursor origin and its motion bounds follow the margins.
///
/// # Control Functions
///
/// - `DECOM` (`CSI ? 6 h` / `CSI ? 6 l`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OriginMode {
    /// Home sits at the top margin, and the cursor cannot move outside
    /// the margins.
    WithinMargins,
    /// Home sits at the upper-left corner of the screen, and the cursor
    /// can move outside the margins. This is the state a power-up or a
    /// reset leaves behind.
    #[default]
    UpperLeftCorner,
}

/// DECSTBM scroll region; `bottom` is the inclusive last row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Margins {
    /// First row of the scroll region (0 = top of screen).
    pub top: ScreenLine,
    /// Inclusive last row of the scroll region (default `rows - 1`).
    pub bottom: ScreenLine,
}

impl Margins {
    /// Builds the default full-screen region for a grid of `rows` rows.
    pub fn new(rows: u16) -> Self {
        Self {
            top: ScreenLine(0),
            bottom: ScreenLine(rows - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(top: u16, bottom: u16, origin_mode: OriginMode) -> ScrollRegion {
        ScrollRegion {
            margins: Margins {
                top: ScreenLine(top),
                bottom: ScreenLine(bottom),
            },
            origin_mode,
        }
    }

    mod scroll_span {
        use super::*;

        /// Asserts that the default region spans the whole page.
        ///
        /// Case: a terminal boots and the shell prints past the last row
        /// before any program has sent DECSTBM.
        #[test]
        fn the_default_region_spans_the_whole_page() {
            assert_eq!(
                ScrollRegion::new(24).scroll_span(),
                ScreenLine(0)..=ScreenLine(23)
            );
        }

        /// Asserts that a set region spans its margin rows and neither
        /// neighbour.
        ///
        /// Case: vim reserves a header at the top of a 24-row page and a
        /// status line at the bottom, and scrolls the rows between them.
        #[test]
        fn a_set_region_holds_both_margin_rows() {
            let span = region(4, 19, OriginMode::UpperLeftCorner).scroll_span();
            assert_eq!(span, ScreenLine(4)..=ScreenLine(19));
            assert!(!span.contains(&ScreenLine(3)));
            assert!(!span.contains(&ScreenLine(20)));
        }
    }
}
