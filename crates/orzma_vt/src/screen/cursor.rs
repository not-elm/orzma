//! The cursor snapshot a screen reports.

use crate::device::modes::CursorShape;
use crate::screen::grid::coords::GridPoint;

/// Bit 0 of the packed `cursor_style` u32 — set when the cursor
/// should be drawn.
pub const CURSOR_VISIBLE_BIT: u32 = 1;

/// Cursor state at snapshot time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The grid position
    pub point: GridPoint,
    /// Visual shape selected by DECSCUSR.
    pub shape: CursorShape,
    /// True when DECSCUSR selects a blinking variant.
    /// Steady variants (`\033[2 q`, `\033[4 q`, `\033[6 q`) set this to false.
    pub blinking: bool,
    /// True when the application wants the cursor drawn, which DECTCEM alone decides.
    pub visible: bool,
}

impl Cursor {
    /// Packs the style into one u32: bit 0 is [`CURSOR_VISIBLE_BIT`],
    /// bits 1-2 carry the shape (Block `0`, Underline `1`, Bar `2`), and
    /// bit 3 carries the blinking flag.
    pub fn pack_cursor_style(&self) -> u32 {
        let visible = if self.visible { CURSOR_VISIBLE_BIT } else { 0 };
        let shape = match self.shape {
            CursorShape::Block => 0u32,
            CursorShape::Underline => 1,
            CursorShape::Bar => 2,
        };
        let blinking = if self.blinking { 1u32 } else { 0 };
        visible | (shape << 1) | (blinking << 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::grid::coords::{GridColumn, GridLine};

    fn cursor(shape: CursorShape, blinking: bool, visible: bool) -> Cursor {
        Cursor {
            point: GridPoint::default(),
            shape,
            blinking,
            visible,
        }
    }

    /// Asserts that every cursor shape occupies its assigned wire bits.
    ///
    /// Case: a terminal application switches among the steady block,
    /// underline, and bar DECSCUSR variants.
    #[test]
    fn each_shape_lands_in_the_shape_bits() {
        assert_eq!(
            cursor(CursorShape::Block, false, true).pack_cursor_style(),
            0b0001
        );
        assert_eq!(
            cursor(CursorShape::Underline, false, true).pack_cursor_style(),
            0b0011
        );
        assert_eq!(
            cursor(CursorShape::Bar, false, true).pack_cursor_style(),
            0b0101
        );
    }

    /// Asserts that blinking is encoded independently for every shape.
    ///
    /// Case: a terminal application selects a blinking caret variant
    /// while the terminal stays focused.
    #[test]
    fn blinking_sets_bit_three_independent_of_shape() {
        assert_eq!(
            cursor(CursorShape::Block, true, true).pack_cursor_style(),
            0b1001
        );
        assert_eq!(
            cursor(CursorShape::Underline, true, true).pack_cursor_style(),
            0b1011
        );
        assert_eq!(
            cursor(CursorShape::Bar, true, true).pack_cursor_style(),
            0b1101
        );
    }

    /// Asserts that a hidden cursor clears only the visible bit,
    /// leaving the packed shape and blink policy intact.
    ///
    /// Case: vim hides the cursor with DECTCEM (`CSI ?25l`) while it
    /// redraws, and on `CSI ?25h` the caret returns.
    #[test]
    fn a_hidden_cursor_clears_only_the_visible_bit() {
        assert_eq!(
            cursor(CursorShape::Bar, true, false).pack_cursor_style(),
            0b1100
        );
        assert_eq!(
            cursor(CursorShape::Block, false, false).pack_cursor_style(),
            0b0000
        );
        assert_eq!(
            cursor(CursorShape::Bar, true, false).pack_cursor_style(),
            cursor(CursorShape::Bar, true, true).pack_cursor_style() & !CURSOR_VISIBLE_BIT
        );
    }

    /// Asserts that the cursor position does not participate in style
    /// packing.
    ///
    /// Case: the user scrolls through history, and the cursor's grid
    /// position projects to a different viewport cell or to no cell at
    /// all.
    #[test]
    fn the_cursor_position_does_not_participate_in_style_packing() {
        let at_origin = Cursor {
            point: GridPoint {
                line: GridLine(0),
                column: GridColumn(0),
            },
            shape: CursorShape::Underline,
            blinking: true,
            visible: true,
        };
        let deep_in_history = Cursor {
            point: GridPoint {
                line: GridLine(-9999),
                column: GridColumn(511),
            },
            ..at_origin
        };
        assert_eq!(
            at_origin.pack_cursor_style(),
            deep_in_history.pack_cursor_style()
        );
    }
}
