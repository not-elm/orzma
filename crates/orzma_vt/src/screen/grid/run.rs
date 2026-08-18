//! One attribute run of an emitted row.
//!
//! The row itself is [`Row`](crate::screen::grid::row::Row); a run
//! is one of its elements.

use crate::schema::{Color, HyperlinkId};

/// Style bitmask bits carried by [`Run::style`].
///
/// The values are pinned by the renderer's shader constants; bits 7-15
/// are reserved.
pub mod style {
    /// Bold weight.
    pub const BOLD: u16 = 1;
    /// Italic slant.
    pub const ITALIC: u16 = 2;
    /// Underline.
    pub const UNDERLINE: u16 = 4;
    /// Strikethrough.
    pub const STRIKE: u16 = 8;
    /// Reverse video (fg/bg swapped).
    pub const REVERSE: u16 = 16;
    /// Faint intensity.
    pub const DIM: u16 = 32;
    /// Hidden (concealed) text.
    pub const HIDDEN: u16 = 64;
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
    /// Style bitmask (see the `style` module). Widened from u8 to u16 so
    /// HIDDEN (bit 6) and future underline variants fit.
    pub style: u16,
    /// UTF-8 text; the consumer uses Unicode East Asian Width to position
    /// each grapheme cluster within the run.
    pub text: String,
    /// Hyperlink id (OSC 8); always `None` until Phase 3.
    pub hyperlink_id: Option<HyperlinkId>,
}
