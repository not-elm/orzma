//! Internal storage cell and the SGR pen burned into it on print.

use crate::device::color::Color;
use crate::hyperlink::HyperlinkId;
use crate::screen::character_sets::GraphicChar;
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
    /// A column the glyph to its left already covers.
    Spacer,
    /// A blank left in the last column because a width-2 glyph did not
    /// fit there; the glyph itself was printed on the next row.
    LeadingSpacer,
}

/// The width a glyph body occupies: the only widths a stamp may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyWidth {
    /// A width-1 glyph.
    Narrow,
    /// A width-2 glyph, followed by its continuation column.
    Wide,
}

impl BodyWidth {
    /// The columns a glyph body of this width spans.
    pub fn columns(self) -> u16 {
        match self {
            Self::Narrow => 1,
            Self::Wide => 2,
        }
    }
}

impl From<BodyWidth> for CellWidth {
    fn from(width: BodyWidth) -> Self {
        match width {
            BodyWidth::Narrow => Self::Narrow,
            BodyWidth::Wide => Self::Wide,
        }
    }
}

/// How a printable character occupies the grid: as a one-column glyph,
/// a two-column glyph, or a mark combined onto the glyph before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphClass {
    /// A glyph one column wide.
    Narrow,
    /// A glyph two columns wide.
    Wide,
    /// A mark that occupies no column of its own.
    ZeroWidth,
}

impl GlyphClass {
    /// Classifies `c`; `None` for a character with no reported width,
    /// such as a control character.
    ///
    /// East Asian Ambiguous characters are [`GlyphClass::Narrow`], and a
    /// reported width above two is [`GlyphClass::Wide`].
    pub fn of(c: char) -> Option<Self> {
        match UnicodeWidthChar::width(c)? {
            0 => Some(Self::ZeroWidth),
            1 => Some(Self::Narrow),
            _ => Some(Self::Wide),
        }
    }

    /// The columns a character of this class advances the cursor by.
    pub fn columns(self) -> u16 {
        match self {
            Self::Narrow => 1,
            Self::Wide => 2,
            Self::ZeroWidth => 0,
        }
    }

    /// The width a body cell of this class stores; `None` for a class
    /// that is never stored as a cell of its own.
    pub fn body_width(self) -> Option<BodyWidth> {
        match self {
            Self::Narrow => Some(BodyWidth::Narrow),
            Self::Wide => Some(BodyWidth::Wide),
            Self::ZeroWidth => None,
        }
    }
}

/// A character already mapped through a character set, paired with the
/// [`GlyphClass`] it prints as.
///
/// # Invariants
///
/// The class is the one [`GlyphClass::of`] reports for the character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassifiedGlyph {
    glyph: char,
    class: GlyphClass,
}

impl ClassifiedGlyph {
    /// Classifies a mapped character; `None` for a character with no
    /// reported width, such as a control character.
    pub fn classify(GraphicChar(glyph): GraphicChar) -> Option<Self> {
        GlyphClass::of(glyph).map(|class| Self { glyph, class })
    }

    /// The mapped character.
    pub fn glyph(self) -> char {
        self.glyph
    }

    /// The class the character prints as.
    pub fn class(self) -> GlyphClass {
        self.class
    }
}

// NOTE: The cap is a memory-exhaustion defense, not a typographic limit:
// a stream that repeats zero-width marks at one cell would otherwise grow
// that cell without bound.
/// How many zero-width marks one cell retains.
pub const MAX_COMBINING: usize = 9;

/// The zero-width marks combined onto a cell's base glyph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellExtra {
    marks: [char; MAX_COMBINING],
    len: u8,
}

impl Default for CellExtra {
    fn default() -> Self {
        Self {
            marks: ['\0'; MAX_COMBINING],
            len: 0,
        }
    }
}

impl CellExtra {
    /// Appends `mark`, reporting whether it was kept; a push past
    /// [`MAX_COMBINING`] is refused and changes nothing.
    pub fn push(&mut self, mark: char) -> bool {
        let len = usize::from(self.len);
        if len >= MAX_COMBINING {
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

    /// The marks combined onto the glyph, in arrival order; empty when
    /// none arrived.
    pub fn marks(&self) -> &[char] {
        self.extra.as_deref().map_or(&[][..], CellExtra::marks)
    }

    /// The glyph followed by the marks combined onto it, in arrival
    /// order; a continuation or filler column yields nothing.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        let body = !matches!(self.width, CellWidth::Spacer | CellWidth::LeadingSpacer);
        body.then_some(self.c)
            .into_iter()
            .chain(self.marks().iter().copied().filter(move |_| body))
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
    /// Burns the pen's attributes into a glyph body holding `c` at
    /// `width`, printed inside `hyperlink_id`, or outside any link when
    /// it is `None`.
    pub fn stamp(&self, c: char, width: BodyWidth, hyperlink_id: Option<HyperlinkId>) -> Cell {
        Cell {
            c,
            width: width.into(),
            extra: None,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
            hyperlink_id,
        }
    }

    /// The blank a width-2 glyph leaves in the last column when it wraps
    /// instead of fitting there, carrying the pen's attributes.
    pub fn filler(&self) -> Cell {
        Cell {
            c: ' ',
            width: CellWidth::LeadingSpacer,
            extra: None,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
            hyperlink_id: None,
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
        for _ in 0..MAX_COMBINING {
            assert!(extra.push('\u{0301}'));
        }
        assert!(!extra.push('\u{0302}'));
        assert_eq!(extra.marks().len(), MAX_COMBINING);
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

    /// Asserts that stamping burns all pen attributes, the given width and
    /// the given hyperlink into the cell.
    ///
    /// Case: an application selects bold red text with SGR and prints a
    /// fullwidth character inside a hyperlink.
    #[test]
    fn stamping_copies_the_pen_attributes_and_the_hyperlink() {
        let pen = Pen {
            fg: Color::Indexed(1),
            bg: Color::Indexed(4),
            style: Style::BOLD,
        };
        let hyperlink_id = HyperlinkId::new(7);
        assert_eq!(
            pen.stamp('あ', BodyWidth::Wide, hyperlink_id),
            Cell {
                c: 'あ',
                width: CellWidth::Wide,
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
    /// mark each classify to their own class.
    ///
    /// Case: a program prints mixed Latin, Japanese and combining text.
    #[test]
    fn glyph_class_classifies_each_class_of_character() {
        assert_eq!(GlyphClass::of('a'), Some(GlyphClass::Narrow));
        assert_eq!(GlyphClass::of('あ'), Some(GlyphClass::Wide));
        assert_eq!(GlyphClass::of('\u{0301}'), Some(GlyphClass::ZeroWidth));
    }

    /// Asserts that a character with no reported width classifies to
    /// `None` rather than to a printable class.
    ///
    /// Case: a program emits an escape byte and a NUL amid printable
    /// text.
    #[test]
    fn a_control_character_has_no_class() {
        assert_eq!(GlyphClass::of('\u{1b}'), None);
        assert_eq!(GlyphClass::of('\0'), None);
    }

    /// Asserts that the one scalar reported as three columns wide
    /// classifies as wide.
    ///
    /// Case: a program prints a character whose reported display width is
    /// three columns.
    #[test]
    fn a_width_three_scalar_classifies_as_wide() {
        assert_eq!(GlyphClass::of('\u{17d8}'), Some(GlyphClass::Wide));
    }

    /// Asserts that a variation selector and a zero-width joiner classify
    /// as zero-width rather than as narrow glyphs.
    ///
    /// Case: a program prints an emoji presentation sequence or a ZWJ
    /// family sequence.
    #[test]
    fn variation_selectors_and_joiners_are_zero_width() {
        assert_eq!(GlyphClass::of('\u{fe0f}'), Some(GlyphClass::ZeroWidth));
        assert_eq!(GlyphClass::of('\u{200d}'), Some(GlyphClass::ZeroWidth));
    }

    /// Asserts that an East Asian Ambiguous character classifies as
    /// narrow.
    ///
    /// Case: a program prints a Greek letter or a box-drawing character
    /// that some CJK locales render fullwidth.
    #[test]
    fn an_ambiguous_width_character_is_narrow() {
        assert_eq!(GlyphClass::of('α'), Some(GlyphClass::Narrow));
        assert_eq!(GlyphClass::of('─'), Some(GlyphClass::Narrow));
    }

    /// Asserts that each class reports its column count and the stored
    /// width a body cell takes, with a zero-width class taking none.
    ///
    /// Case: the printer decides how far to advance and what to stamp for
    /// each class of character.
    #[test]
    fn each_class_reports_its_columns_and_body_width() {
        assert_eq!(GlyphClass::Narrow.columns(), 1);
        assert_eq!(GlyphClass::Wide.columns(), 2);
        assert_eq!(GlyphClass::ZeroWidth.columns(), 0);
        assert_eq!(GlyphClass::Narrow.body_width(), Some(BodyWidth::Narrow));
        assert_eq!(GlyphClass::Wide.body_width(), Some(BodyWidth::Wide));
        assert_eq!(GlyphClass::ZeroWidth.body_width(), None);
    }

    /// Asserts that each body width reports the columns its glyph spans.
    ///
    /// Case: the printer makes room for a glyph it is about to stamp and
    /// then advances the cursor past it.
    #[test]
    fn each_body_width_reports_its_columns() {
        assert_eq!(BodyWidth::Narrow.columns(), 1);
        assert_eq!(BodyWidth::Wide.columns(), 2);
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

    /// Asserts that a stamped cell stores the given body width and
    /// carries no marks.
    ///
    /// Case: an application prints an ASCII letter and then a kanji.
    #[test]
    fn stamping_produces_a_cell_of_the_given_width() {
        let narrow = Pen::default().stamp('a', BodyWidth::Narrow, None);
        assert_eq!(narrow.width, CellWidth::Narrow);
        assert_eq!(narrow.extra, None);
        let wide = Pen::default().stamp('界', BodyWidth::Wide, None);
        assert_eq!(wide.width, CellWidth::Wide);
        assert_eq!(wide.extra, None);
    }

    /// Asserts that a cell yields its glyph followed by its marks in
    /// arrival order, a cell without marks yields the glyph alone, and a
    /// continuation column yields nothing.
    ///
    /// Case: a copy walks a row holding an accented letter next to a blank
    /// cell and a wide glyph's continuation column.
    #[test]
    fn a_cell_yields_its_glyph_and_then_its_marks() {
        let mut extra = CellExtra::default();
        assert!(extra.push('\u{0302}'));
        assert!(extra.push('\u{0301}'));
        let accented = Cell {
            c: 'e',
            extra: Some(Box::new(extra)),
            ..Cell::default()
        };
        assert_eq!(accented.chars().collect::<String>(), "e\u{0302}\u{0301}");
        assert_eq!(Cell::default().chars().collect::<String>(), " ");
        assert_eq!(
            Cell {
                width: CellWidth::Spacer,
                ..Cell::default()
            }
            .chars()
            .count(),
            0
        );
        let mut spacer_extra = CellExtra::default();
        assert!(spacer_extra.push('\u{0301}'));
        assert_eq!(
            Cell {
                width: CellWidth::Spacer,
                extra: Some(Box::new(spacer_extra)),
                ..Cell::default()
            }
            .chars()
            .count(),
            0
        );
    }
}
