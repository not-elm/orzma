//! `Scroll`: the viewport motion vocabulary a VT applies to its grid.

#[cfg(feature = "alacritty")]
use alacritty_terminal::grid::Scroll as AlacrittyScroll;

/// A viewport motion over the scrollback, clamped by the VT at both
/// the oldest retained line and the live tail.
///
/// `Delta` is signed: positive moves toward older output (deeper into
/// scrollback), negative toward the live tail. Page-sized variants are
/// resolved against the live grid height by the VT backend, so callers
/// never need to know the row count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Moves by a signed line count: positive toward older output,
    /// negative toward the live tail.
    Delta(i32),
    /// One screenful toward older output.
    PageUp,
    /// One screenful toward the live tail.
    PageDown,
    /// Half a screenful toward older output.
    HalfPageUp,
    /// Half a screenful toward the live tail.
    HalfPageDown,
    /// The oldest line still in scrollback.
    Top,
    /// The live tail.
    Bottom,
}

impl Scroll {
    /// Converts to alacritty's `Scroll`, resolving the half-page
    /// variants — which alacritty's enum lacks — into a `Delta` of
    /// `screen_lines / 2` rows.
    #[cfg(feature = "alacritty")]
    pub fn to_alacritty_scroll(&self, screen_lines: u16) -> AlacrittyScroll {
        let half_page = i32::from(screen_lines / 2);
        match self {
            Self::Delta(n) => AlacrittyScroll::Delta(*n),
            Self::PageUp => AlacrittyScroll::PageUp,
            Self::PageDown => AlacrittyScroll::PageDown,
            Self::HalfPageUp => AlacrittyScroll::Delta(half_page),
            Self::HalfPageDown => AlacrittyScroll::Delta(-half_page),
            Self::Top => AlacrittyScroll::Top,
            Self::Bottom => AlacrittyScroll::Bottom,
        }
    }
}
