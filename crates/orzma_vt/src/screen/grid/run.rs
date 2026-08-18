//! One attribute run of an emitted row.
//!
//! The row itself is [`Row`](crate::screen::grid::row::Row); a run
//! is one of its elements.

use crate::schema::{Color, HyperlinkId};
use bitflags::bitflags;

bitflags! {
    /// The SGR attributes a cell or a run carries.
    ///
    /// # Invariants
    ///
    /// The bit values are pinned by the renderer's shader constants,
    /// which read the raw bits: renumbering a flag silently repaints
    /// every cell with the wrong attribute. Bits 7-15 are reserved for
    /// the underline variants SGR 4:2-4:5 adds.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
    pub struct Style: u16 {
        /// Bold weight.
        const BOLD = 1;
        /// Italic slant.
        const ITALIC = 1 << 1;
        /// Underline.
        const UNDERLINE = 1 << 2;
        /// Strikethrough.
        const STRIKE = 1 << 3;
        /// Reverse video (fg/bg swapped).
        const REVERSE = 1 << 4;
        /// Faint intensity.
        const DIM = 1 << 5;
        /// Hidden (concealed) text.
        const HIDDEN = 1 << 6;
    }
}

/// A run of cells sharing identical fg/bg/style attributes.
///
/// Wide-char spacers (alacritty internal) are absorbed by this crate and
/// do not appear in `text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Total column span (sum of grapheme cluster widths in `text`).
    pub cols: u16,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// The SGR attributes every cell in the run shares.
    pub style: Style,
    /// UTF-8 text; the consumer uses Unicode East Asian Width to position
    /// each grapheme cluster within the run.
    pub text: String,
    /// Hyperlink id (OSC 8); always `None` until Phase 3.
    pub hyperlink_id: Option<HyperlinkId>,
}
