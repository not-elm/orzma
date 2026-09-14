//! One attribute run of an emitted row.

use crate::device::color::Color;
use crate::error::{RunError, VtResult};
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
    /// A run carrying `cell`'s foreground, background, style, and
    /// hyperlink, spanning no columns yet.
    pub(crate) fn opened_by(cell: &Cell) -> Self {
        Self {
            cols: 0,
            fg: cell.fg,
            bg: cell.bg,
            style: cell.style,
            text: String::new(),
            widths: Vec::new(),
            hyperlink_id: cell.hyperlink_id,
        }
    }

    /// Checks that `widths` describes `text` and `cols`.
    ///
    /// An empty `widths` passes on its own: it stands for one column per
    /// `char`, and `cols` is not compared against the `char` count.
    ///
    /// # Errors
    ///
    /// [`RunError::WidthCount`] when a non-empty `widths` has an entry
    /// count other than the `char` count of `text`;
    /// [`RunError::WidthSum`] when a non-empty `widths` does not sum to
    /// `cols`; [`RunError::InvalidWidth`] when an entry is above 2; and
    /// [`RunError::LeadingContinuation`] when the first entry is 0.
    pub fn check(&self) -> VtResult {
        if !self.widths.is_empty() {
            if self.widths.len() != self.text.chars().count() {
                return Err(RunError::WidthCount.into());
            }
            let sum: u32 = self.widths.iter().map(|w| u32::from(*w)).sum();
            if sum != u32::from(self.cols) {
                return Err(RunError::WidthSum.into());
            }
        }
        if self.widths.iter().any(|w| *w > 2) {
            return Err(RunError::InvalidWidth.into());
        }
        if self.widths.first() == Some(&0) {
            return Err(RunError::LeadingContinuation.into());
        }
        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::VtError;

    fn run(text: &str, cols: u16, widths: &[u8]) -> Run {
        Run {
            cols,
            text: text.to_string(),
            widths: widths.to_vec(),
            ..Run::default()
        }
    }

    /// Asserts that a run without widths passes whatever its column
    /// count is.
    ///
    /// Case: the VT emits an ASCII row on the path that allocates no
    /// widths.
    #[test]
    fn a_run_without_widths_passes() {
        run("ab", 2, &[]).check().expect("a valid run");
    }

    /// Asserts that widths covering each `char` and summing to the
    /// column count pass.
    ///
    /// Case: the VT emits a wide glyph, a marked letter and a narrow
    /// glyph in one run.
    #[test]
    fn widths_that_describe_the_text_pass() {
        run("\u{3042}e\u{0301}b", 4, &[2, 1, 0, 1])
            .check()
            .expect("a valid run");
    }

    /// Asserts that a widths list shorter than the text is refused as
    /// `WidthCount`.
    ///
    /// Case: a producer appends a `char` to a run without extending its
    /// widths.
    #[test]
    fn a_widths_list_of_the_wrong_length_is_refused() {
        assert!(matches!(
            run("ab", 2, &[1]).check(),
            Err(VtError::Run(RunError::WidthCount))
        ));
    }

    /// Asserts that widths that do not sum to the column count are
    /// refused as `WidthSum`.
    ///
    /// Case: a producer records a wide glyph's width without adding its
    /// second column to the run.
    #[test]
    fn widths_that_do_not_sum_to_the_columns_are_refused() {
        assert!(matches!(
            run("\u{3042}", 1, &[2]).check(),
            Err(VtError::Run(RunError::WidthSum))
        ));
    }

    /// Asserts that a width above two is refused as `InvalidWidth`.
    ///
    /// Case: a producer writes a glyph's byte length where its column
    /// span belongs.
    #[test]
    fn a_width_above_two_is_refused() {
        assert!(matches!(
            run("\u{3042}", 3, &[3]).check(),
            Err(VtError::Run(RunError::InvalidWidth))
        ));
    }

    /// Asserts that a run whose first width is zero is refused as
    /// `LeadingContinuation`.
    ///
    /// Case: a run boundary falls between a letter and its combining
    /// mark.
    #[test]
    fn a_leading_continuation_is_refused() {
        assert!(matches!(
            run("\u{0301}a", 1, &[0, 1]).check(),
            Err(VtError::Run(RunError::LeadingContinuation))
        ));
    }
}
