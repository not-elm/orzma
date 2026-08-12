//! Vocabulary for a row of attribute runs.

use crate::schema::{Color, HyperlinkId};

/// A row of runs ordered left-to-right.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Runs in left-to-right column order.
    pub runs: Vec<Run>,
}

/// A run of cells sharing identical fg/bg/style attributes.
///
/// Wide-char spacers (alacritty internal) are absorbed server-side and do
/// not appear in `text`.
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
    /// UTF-8 text; the client uses Unicode East Asian Width to position each
    /// grapheme cluster within the run.
    pub text: String,
    /// Hyperlink id (OSC 8); always `None` until Phase 3.
    pub hyperlink_id: Option<HyperlinkId>,
}
