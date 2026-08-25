//! Internal storage cell and the SGR pen burned into it on print.

use crate::device::color::Color;
use crate::screen::grid::run::Style;

/// One stored character cell: a glyph plus the attributes it was
/// printed with.
///
/// This is the storage representation, never exposed outside the
/// crate: it carries no coordinate (position is implied by the cell's
/// slot in the grid) and holds a single `char` (grapheme composition
/// is a later extension). The host reads cells only through the
/// emitted [`crate::screen::grid::row::Row`] / [`crate::screen::grid::run::Run`] projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The stored glyph.
    pub c: char,
    /// Foreground color, symbolic (palette-resolved at emit time).
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
    /// Case: a terminal spawns with an untouched screen, whose cells
    /// must render as the terminal's default background.
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
    /// printing, and every printed cell carries those attributes.
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
}
