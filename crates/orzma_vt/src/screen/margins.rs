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

    /// Returns the cursor origin.
    pub fn origin_mode(&self) -> OriginMode {
        self.origin_mode
    }

    /// Replaces the cursor origin, and does nothing else.
    ///
    /// This is the plain assignment `DECRC` needs to put a saved mode
    /// back. `DECOM` itself additionally homes the cursor, which this
    /// type cannot do because it does not own one; that half belongs to
    /// the `Screen` method the control function reaches.
    pub fn set_origin_mode(&mut self, origin_mode: OriginMode) {
        self.origin_mode = origin_mode;
    }

    /// The rows a scroll moves: the top margin through the bottom
    /// margin, inclusive.
    ///
    /// The span never consults [`OriginMode`], because DECSTBM confines
    /// scrolling to the margins whichever way the origin is set.
    pub fn scroll_span(&self) -> RangeInclusive<ScreenLine> {
        self.margins.top..=self.margins.bottom
    }

    /// The margins, for a caller that needs to compare them as a pair.
    pub(crate) fn margins(&self) -> Margins {
        self.margins
    }

    /// Replaces the margins, and does nothing else.
    ///
    /// This is the plain assignment `DECSTBM` needs. Seating the cursor
    /// at the new home is the other half of that control function and
    /// belongs to the `Screen` method that reaches it, because this type
    /// owns no cursor.
    pub(crate) fn set_margins(&mut self, margins: Margins) {
        self.margins = margins;
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
pub(crate) struct Margins {
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

    /// Resolves the `DECSTBM` parameters against a page of `rows` rows;
    /// `None` for a request that must be refused.
    ///
    /// Both parameters are one-based line numbers as sent, with `None`
    /// for an omitted one. A zero means the default, the same as an
    /// omission.
    ///
    /// # Control Functions
    ///
    /// - `DECSTBM` (`CSI Pt ; Pb r`)
    pub(crate) fn resolve(top: Option<u16>, bottom: Option<u16>, rows: u16) -> Option<Self> {
        let top = match top {
            None | Some(0) => 1,
            Some(line) => line,
        };
        let bottom = match bottom {
            None | Some(0) => rows,
            Some(line) => line.min(rows),
        };
        if top >= bottom {
            return None;
        }
        Some(Self {
            top: ScreenLine(top - 1),
            bottom: ScreenLine(bottom - 1),
        })
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

    mod resolve {
        use super::*;

        /// Asserts that two omitted parameters resolve to the whole
        /// page.
        ///
        /// Case: an application drops a scroll region it had set by
        /// sending a bare `CSI r`.
        #[test]
        fn omitted_parameters_resolve_to_the_whole_page() {
            assert_eq!(
                Margins::resolve(None, None, 24),
                Some(Margins {
                    top: ScreenLine(0),
                    bottom: ScreenLine(23),
                })
            );
        }

        /// Asserts that a zero resolves the same way an omission does.
        ///
        /// The agreed policy follows ECMA-48 §5.4.2's default rule and
        /// xterm's `one_if_default`, which folds any value at or below
        /// zero to the default. VT510 does not define a zero here.
        ///
        /// Case: a program that builds its sequences from unset integer
        /// variables emits `CSI 0 ; 0 r`.
        #[test]
        fn a_zero_resolves_like_an_omission() {
            assert_eq!(
                Margins::resolve(Some(0), Some(0), 24),
                Margins::resolve(None, None, 24)
            );
        }

        /// Asserts that one-based parameters land on zero-based rows
        /// with both ends inside the region.
        ///
        /// Case: vim reserves the first row for a header and the last
        /// for a status line on a 24-row page, sending `CSI 2 ; 23 r`.
        #[test]
        fn one_based_parameters_land_on_zero_based_rows() {
            assert_eq!(
                Margins::resolve(Some(2), Some(23), 24),
                Some(Margins {
                    top: ScreenLine(1),
                    bottom: ScreenLine(22),
                })
            );
        }

        /// Asserts that a bottom margin past the last row clamps to it.
        ///
        /// The agreed policy clamps rather than refusing, because VT510
        /// p.276 describes the page size as the region's maximum rather
        /// than as a precondition. Windows Terminal refuses instead.
        ///
        /// Case: an application sized for a taller window asks for a
        /// region reaching row 40 on a 24-row screen.
        #[test]
        fn a_bottom_past_the_last_row_clamps() {
            assert_eq!(
                Margins::resolve(Some(1), Some(40), 24),
                Some(Margins {
                    top: ScreenLine(0),
                    bottom: ScreenLine(23),
                })
            );
        }

        /// Asserts that a top margin at or below the bottom margin
        /// refuses the request.
        ///
        /// The agreed policy refuses rather than repairing the request:
        /// VT510 p.276 requires "The value of the top margin (Pt) must
        /// be less than the bottom margin (Pb)", and clamping into the
        /// nearest legal region would leave the application drawing
        /// somewhere it never asked for.
        ///
        /// Case: an application inverts its two parameters and sends
        /// `CSI 5 ; 3 r`.
        #[test]
        fn an_inverted_region_is_refused() {
            assert_eq!(Margins::resolve(Some(5), Some(3), 24), None);
        }

        /// Asserts that a single-row region is refused.
        ///
        /// Case: an application computes equal margins and sends
        /// `CSI 3 ; 3 r`.
        #[test]
        fn a_single_row_region_is_refused() {
            assert_eq!(Margins::resolve(Some(3), Some(3), 24), None);
        }

        /// Asserts that the clamp runs before the comparison, so a
        /// bottom that clamps below the top is refused.
        ///
        /// The agreed policy orders it the way xterm does — default,
        /// clamp, then compare. alacritty compares first, which lets
        /// this request through and collapses it into a degenerate
        /// region.
        ///
        /// Case: an application sized for a taller window asks for rows
        /// 30 through 40 on a 24-row screen.
        #[test]
        fn a_bottom_that_clamps_below_the_top_is_refused() {
            assert_eq!(Margins::resolve(Some(30), Some(40), 24), None);
        }
    }
}
