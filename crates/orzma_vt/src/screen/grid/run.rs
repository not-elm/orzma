//! One attribute run of an emitted row.

use crate::device::color::Color;
use crate::hyperlink::HyperlinkId;
use crate::screen::cell::Cell;
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
    /// Total column span.
    pub cols: u16,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// The SGR attributes every cell in the run shares.
    pub style: Style,
    /// UTF-8 text.
    pub text: String,
    /// The column span each `char` of `text` opens: `1` or `2` for a glyph,
    /// `0` for a mark combined onto the glyph before it.
    ///
    /// Empty when every `char` is a one-column glyph, in which case
    /// `cols` equals the `char` count; otherwise one entry per `char`
    /// summing to `cols`.
    pub widths: Vec<u8>,
    /// The hyperlink every cell in the run carries, if any. It resolves
    /// against the definitions the frame carries.
    pub hyperlink_id: Option<HyperlinkId>,
}

impl Run {
    /// Whether `cell` carries this run's foreground, background, style, and
    /// hyperlink, so it extends the run rather than starting a new one.
    pub fn continues_with(&self, cell: &Cell) -> bool {
        self.fg == cell.fg
            && self.bg == cell.bg
            && self.style == cell.style
            && self.hyperlink_id == cell.hyperlink_id
    }
}

impl Default for Run {
    fn default() -> Self {
        Self {
            cols: Default::default(),
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Default::default(),
            text: Default::default(),
            widths: Default::default(),
            hyperlink_id: Default::default(),
        }
    }
}
