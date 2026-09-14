//! Internal storage cell and the SGR pen burned into it on print.

use crate::device::color::Color;
use crate::hyperlink::HyperlinkId;
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
    LeadingSpacer,
}

impl CellWidth {
    /// Classifies `c` by the columns it occupies; `None` for a character
    /// with no reported width, such as a control character.
    ///
    /// A width above two is reported as [`CellWidth::Wide`].
    pub fn of(c: char) -> Option<Self> {
        match UnicodeWidthChar::width(c)? {
            0 => Some(Self::Spacer),
            1 => Some(Self::Narrow),
            _ => Some(Self::Wide),
        }
    }
}

/// The zero-width marks combined onto a cell's base glyph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellExtra {
    marks: [char; CellExtra::MAX_COMBINING],
    len: u8,
}

impl Default for CellExtra {
    fn default() -> Self {
        Self {
            marks: ['\0'; Self::MAX_COMBINING],
            len: 0,
        }
    }
}

impl CellExtra {
    // NOTE: The cap is a memory-exhaustion defense, not a typographic
    // limit: a stream that repeats zero-width marks at one cell would
    // otherwise grow that cell without bound.
    /// How many zero-width marks one cell retains.
    pub const MAX_COMBINING: usize = 9;

    /// Appends `mark`, reporting whether it was kept; a push past
    /// [`CellExtra::MAX_COMBINING`] is refused and changes nothing.
    pub fn push(&mut self, mark: char) -> bool {
        let len = usize::from(self.len);
        if len >= Self::MAX_COMBINING {
            return false;
        }
        self.marks[len] = mark;
        self.len += 1;
        true
    }

    /// The marks this cell carries, in the order they arrived.
    pub fn marks(&self) -> &[char] {
        &self.marks[..usize::from(self.len)]
    }
}

/// One stored character cell: a glyph plus the attributes it was
/// printed with.
///
/// TODO: hold a grapheme cluster rather than a single `char`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The stored glyph.
    pub c: char,
    /// The columns this cell occupies, and whether it is a body or a
    /// continuation column.
    pub width: CellWidth,
    /// The zero-width marks combined onto the glyph, when any arrived.
    pub extra: Option<Box<CellExtra>>,
    /// Foreground color, symbolic.
    pub fg: Color,
    /// Background color, symbolic.
    pub bg: Color,
    /// The SGR attributes the glyph was printed with.
    pub style: Style,
    /// The hyperlink the glyph was printed inside, if any.
    pub hyperlink_id: Option<HyperlinkId>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            width: CellWidth::Narrow,
            extra: None,
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Style::empty(),
            hyperlink_id: None,
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

    /// The continuation cell that follows a [`CellWidth::Wide`] body,
    /// sharing its pen.
    pub fn continuation(&self) -> Self {
        Self {
            width: CellWidth::Spacer,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
            hyperlink_id: self.hyperlink_id,
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
    /// Burns the pen's attributes into a cell holding `c`, printed inside
    /// `hyperlink_id`, or outside any link when it is `None`.
    pub fn stamp(&self, c: char, hyperlink_id: Option<HyperlinkId>) -> Cell {
        Cell {
            c,
            width: CellWidth::Narrow,
            extra: None,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
            hyperlink_id,
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

    /// Asserts that pushed marks are returned in the order they arrived.
    ///
    /// Case: a program prints a base letter followed by two combining
    /// accents.
    #[test]
    fn marks_come_back_in_push_order() {
        let mut extra = CellExtra::default();
        assert!(extra.push('\u{0302}'));
        assert!(extra.push('\u{0301}'));
        assert_eq!(extra.marks(), ['\u{0302}', '\u{0301}']);
    }

    /// Asserts that a push past the cap is refused and leaves the kept
    /// marks untouched.
    ///
    /// Case: a stream sends a long run of combining marks at one cell.
    #[test]
    fn a_push_past_the_cap_is_refused() {
        let mut extra = CellExtra::default();
        for _ in 0..CellExtra::MAX_COMBINING {
            assert!(extra.push('\u{0301}'));
        }
        assert!(!extra.push('\u{0302}'));
        assert_eq!(extra.marks().len(), CellExtra::MAX_COMBINING);
        assert!(extra.marks().iter().all(|mark| *mark == '\u{0301}'));
    }

    /// Asserts that a fresh extra holds no marks.
    ///
    /// Case: a cell is allocated an extra before any mark arrives.
    #[test]
    fn a_fresh_extra_is_empty() {
        assert_eq!(CellExtra::default().marks(), [] as [char; 0]);
    }

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

    /// Asserts that stamping burns all pen attributes and the given
    /// hyperlink into the cell.
    ///
    /// Case: an application selects bold red text with SGR and prints it
    /// inside a hyperlink.
    #[test]
    fn stamping_copies_the_pen_attributes_and_the_hyperlink() {
        let pen = Pen {
            fg: Color::Indexed(1),
            bg: Color::Indexed(4),
            style: Style::BOLD,
        };
        let hyperlink_id = HyperlinkId::new(7);
        assert_eq!(
            pen.stamp('a', hyperlink_id),
            Cell {
                c: 'a',
                width: CellWidth::Narrow,
                extra: None,
                fg: Color::Indexed(1),
                bg: Color::Indexed(4),
                style: Style::BOLD,
                hyperlink_id,
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
    /// Case: a program emits an escape byte and a NUL amid printable
    /// text.
    #[test]
    fn a_control_character_has_no_width() {
        assert_eq!(CellWidth::of('\u{1b}'), None);
        assert_eq!(CellWidth::of('\0'), None);
    }

    /// Asserts that the one scalar reported as three columns wide is
    /// clamped to two.
    ///
    /// Case: a program prints a character whose reported display width is
    /// three columns.
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

    /// Asserts that the cell stays within thirty-two bytes.
    ///
    /// Case: a scrollback of ten thousand rows holds millions of cells.
    #[test]
    fn the_cell_stays_within_its_size_budget() {
        assert!(size_of::<Cell>() <= 32, "{}", size_of::<Cell>());
    }

    /// Asserts that a continuation cell is a blank sharing the body's
    /// pen.
    ///
    /// Case: a fullwidth glyph is printed inside a region with a colored
    /// background.
    #[test]
    fn a_continuation_shares_the_body_pen() {
        let body = Cell {
            c: 'あ',
            width: CellWidth::Wide,
            extra: None,
            fg: Color::Indexed(1),
            bg: Color::Indexed(4),
            style: Style::BOLD,
            hyperlink_id: None,
        };
        let spacer = body.continuation();
        assert_eq!(spacer.width, CellWidth::Spacer);
        assert_eq!(spacer.c, ' ');
        assert_eq!(spacer.extra, None);
        assert_eq!(
            (spacer.fg, spacer.bg, spacer.style),
            (body.fg, body.bg, body.style)
        );
    }

    /// Asserts that a stamped cell is narrow and carries no marks.
    ///
    /// Case: an application prints ordinary text.
    #[test]
    fn stamping_produces_a_narrow_cell() {
        let cell = Pen::default().stamp('a', None);
        assert_eq!(cell.width, CellWidth::Narrow);
        assert_eq!(cell.extra, None);
    }
}
