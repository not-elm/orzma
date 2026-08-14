//! Cursor vocabulary: the live cursor and the vi-mode cursor.

use crate::schema::{GridPoint, ViewportPoint};

/// Bit 0 of the packed `cursor_style` u32 — set when the cursor
/// should be drawn. The WGSL shader short-circuits when this bit is
/// clear (see `terminal_ui_material.wgsl:337`). Exposed so app-level
/// overrides (e.g., `TerminalGrid.suppress_cursor`) can mask it out
/// without re-deriving the literal `1`.
pub const CURSOR_VISIBLE_BIT: u32 = 1;

/// Vi-mode cursor position in viewport coordinates.
///
/// The VT keeps the vi cursor inside the visible viewport via
/// `Term::scroll_display`. `in_scrollback` is the safety valve: when
/// `true`, the cursor sits above the viewport, the renderer skips it,
/// and `point.row` is clamped to `-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViCursor {
    /// Viewport cell the vi cursor sits on. `row` is `-1` when
    /// `in_scrollback` is true.
    pub point: ViewportPoint,
    /// True when the vi cursor is above the viewport (in scrollback).
    pub in_scrollback: bool,
}

/// Cursor state at snapshot time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The grid position
    pub point: GridPoint,
    /// Visual shape selected by DECSCUSR.
    pub shape: CursorShape,
    /// True when DECSCUSR selects a blinking variant.
    /// Steady variants (`\033[2 q`, `\033[4 q`, `\033[6 q`) set this to false.
    pub blinking: bool,
    /// True when the application wants the cursor drawn — DECTCEM
    /// (`TermMode::SHOW_CURSOR`) and a non-Hidden DECSCUSR shape.
    /// Scroll visibility is not folded in: project `point` with
    /// [`crate::schema::GridLine::to_viewport`] to decide whether
    /// there is a cell to paint at all.
    pub visible: bool,
}

impl Cursor {
    /// Packs the style into the u32 the WGSL shader decodes: bit 0 is
    /// [`CURSOR_VISIBLE_BIT`], bits 1-2 carry the shape (Block `0`,
    /// Underline `1`, Bar `2`), and bit 3 carries the blinking flag
    /// (see `terminal_ui_material.wgsl:99-103`).
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

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// Block cursor.
    #[default]
    Block,
    /// Underline cursor.
    Underline,
    /// Bar (vertical line) cursor.
    Bar,
}

#[cfg(feature = "alacritty")]
impl From<alacritty_terminal::vte::ansi::CursorShape> for CursorShape {
    fn from(value: alacritty_terminal::vte::ansi::CursorShape) -> Self {
        use alacritty_terminal::vte::ansi::CursorShape as C;
        match value {
            C::Underline => CursorShape::Underline,
            C::Beam => CursorShape::Bar,
            _ => CursorShape::Block,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{GridColumn, GridLine};

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
    /// underline, and bar DECSCUSR variants, and the selected caret
    /// shape is forwarded to the GPU.
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
    /// while the terminal stays focused, so the shader's time-based
    /// blink phase controls whether the caret is painted.
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
    /// redraws, and on `CSI ?25h` the caret returns with the same
    /// shape and blink policy the application had selected.
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
    /// Case: the user scrolls through history while the live cursor
    /// keeps its application-selected shape and blink policy, even as
    /// its grid position projects to a different viewport cell or to
    /// no cell at all.
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
            ..at_origin.clone()
        };
        assert_eq!(
            at_origin.pack_cursor_style(),
            deep_in_history.pack_cursor_style()
        );
    }
}
