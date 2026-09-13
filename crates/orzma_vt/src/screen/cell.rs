//! Internal storage cell and the SGR pen burned into it on print.

use crate::device::color::Color;
use crate::screen::grid::run::Style;
use unicode_width::UnicodeWidthChar;

/// How many columns a cell occupies, and whether it is a body or a
/// continuation column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellWidth {
    /// A width-1 glyph.
    #[default]
    Narrow,
    /// The body of a width-2 glyph; the column to its right holds
    /// [`CellWidth::Spacer`].
    Wide,
    /// A column the glyph to its left already covers, and the class of a
    /// zero-width mark.
    Spacer,
    /// A blank left in the last column because a width-2 glyph did not
    /// fit there; the glyph itself was printed on the next row.
    #[expect(
        dead_code,
        reason = "the printer reaches the classifier when width dispatch lands"
    )]
    LeadingSpacer,
}

impl CellWidth {
    /// Classifies `c` by the columns it occupies; `None` for a character
    /// with no reported width, such as a control character.
    ///
    /// A width above two is reported as [`CellWidth::Wide`].
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the printer reaches the classifier when width dispatch lands"
        )
    )]
    pub fn of(c: char) -> Option<Self> {
        match UnicodeWidthChar::width(c)? {
            0 => Some(Self::Spacer),
            1 => Some(Self::Narrow),
            _ => Some(Self::Wide),
        }
    }
}

/// One stored character cell: a glyph plus the attributes it was
/// printed with.
///
/// TODO: hold a grapheme cluster rather than a single `char`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The stored glyph.
    pub c: char,
    /// Foreground color, symbolic.
    pub fg: Color,
    /// Background color, symbolic.
    pub bg: Color,
    /// The SGR attributes the glyph was printed with.
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Style::empty(),
        }
    }
}

impl Cell {
    /// Builds a blank cell carrying only the given background (BCE).
    pub fn blank_with_bg(bg: Color) -> Self {
        Self {
            bg,
            ..Self::default()
        }
    }
}

/// The current SGR attributes applied to subsequently printed cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pen {
    /// Foreground selected by SGR 30-38/39/90-97.
    pub fg: Color,
    /// Background selected by SGR 40-48/49/100-107.
    pub bg: Color,
    /// The SGR attributes accumulated from SGR sequences.
    pub style: Style,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Style::empty(),
        }
    }
}

impl Pen {
    /// Burns the pen's attributes into a cell holding `c`.
    pub fn stamp(&self, c: char) -> Cell {
        Cell {
            c,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
        }
    }

    /// Builds the blank cell erase operations write: the pen's
    /// background with default foreground and no styling (BCE).
    pub fn erase_cell(&self) -> Cell {
        Cell::blank_with_bg(self.bg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the default cell is a blank space with default
    /// colors and no styling.
    ///
    /// Case: a terminal spawns with an untouched screen.
    #[test]
    fn the_default_cell_is_a_default_colored_blank() {
        let cell = Cell::default();
        assert_eq!(cell.c, ' ');
        assert_eq!(cell.fg, Color::DefaultForeground);
        assert_eq!(cell.bg, Color::DefaultBackground);
        assert_eq!(cell.style, Style::empty());
    }

    /// Asserts that stamping burns all pen attributes into the cell.
    ///
    /// Case: an application selects bold red text with SGR before
    /// printing.
    #[test]
    fn stamping_copies_the_pen_attributes() {
        let pen = Pen {
            fg: Color::Indexed(1),
            bg: Color::Indexed(4),
            style: Style::BOLD,
        };
        assert_eq!(
            pen.stamp('a'),
            Cell {
                c: 'a',
                fg: Color::Indexed(1),
                bg: Color::Indexed(4),
                style: Style::BOLD,
            }
        );
    }

    /// Asserts that the erase cell keeps only the pen's background.
    ///
    /// Case: an application sets a colored background and clears a
    /// region of the screen.
    #[test]
    fn the_erase_cell_keeps_only_the_background() {
        let pen = Pen {
            fg: Color::Indexed(1),
            bg: Color::Indexed(4),
            style: Style::BOLD,
        };
        assert_eq!(pen.erase_cell(), Cell::blank_with_bg(Color::Indexed(4)));
        assert_eq!(pen.erase_cell().fg, Color::DefaultForeground);
        assert_eq!(pen.erase_cell().style, Style::empty());
    }

    /// Asserts that a narrow glyph, a fullwidth glyph and a zero-width
    /// mark each classify to their own width.
    ///
    /// Case: a program prints mixed Latin, Japanese and combining text.
    #[test]
    fn cell_width_classifies_each_class_of_character() {
        assert_eq!(CellWidth::of('a'), Some(CellWidth::Narrow));
        assert_eq!(CellWidth::of('あ'), Some(CellWidth::Wide));
        assert_eq!(CellWidth::of('\u{0301}'), Some(CellWidth::Spacer));
    }

    /// Asserts that a character with no reported width classifies to
    /// `None` rather than to a printable width.
    ///
    /// Case: an escape byte reaches the classifier through a path that
    /// did not strip control characters.
    #[test]
    fn a_control_character_has_no_width() {
        assert_eq!(CellWidth::of('\u{1b}'), None);
        assert_eq!(CellWidth::of('\0'), None);
    }

    /// Asserts that the one scalar reported as three columns wide is
    /// clamped to two.
    ///
    /// Case: a program prints U+17D8, the only scalar in Unicode whose
    /// reported width exceeds two.
    #[test]
    fn a_width_three_scalar_is_clamped_to_wide() {
        assert_eq!(CellWidth::of('\u{17d8}'), Some(CellWidth::Wide));
    }

    /// Asserts that a variation selector and a zero-width joiner are
    /// classified as zero-width rather than as narrow glyphs.
    ///
    /// Case: a program prints an emoji presentation sequence or a ZWJ
    /// family sequence.
    #[test]
    fn variation_selectors_and_joiners_are_zero_width() {
        assert_eq!(CellWidth::of('\u{fe0f}'), Some(CellWidth::Spacer));
        assert_eq!(CellWidth::of('\u{200d}'), Some(CellWidth::Spacer));
    }
}
