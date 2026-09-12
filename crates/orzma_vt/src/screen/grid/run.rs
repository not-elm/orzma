//! One attribute run of an emitted row.

use crate::device::color::Color;
use crate::hyperlink::HyperlinkId;
use bitflags::bitflags;

bitflags! {
    /// The SGR attributes a cell or a run carries.
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
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Total column span: one column per `char` in `text`.
    pub cols: u16,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// The SGR attributes every cell in the run shares.
    pub style: Style,
    /// UTF-8 text.
    pub text: String,
    /// Hyperlink id (OSC 8); it is always `None`.
    ///
    /// TODO: set it once OSC 8 handling reaches the hyperlink interner.
    pub hyperlink_id: Option<HyperlinkId>,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            cols: Default::default(),
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Default::default(),
            text: Default::default(),
            hyperlink_id: Default::default(),
        }
    }
}
