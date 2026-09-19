//! What one frame paints for the caret, and the packed `cursor_style`
//! word the shader decodes it from.

use bevy::prelude::*;
use bitflags::bitflags;
use orzma_vt::prelude::{Cursor, CursorShape};

bitflags! {
    /// The packed `cursor_style` word the shader decodes, with the shape
    /// carried in bits 1-2.
    ///
    /// # Invariants
    ///
    /// - At most one of the shape bits is set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PackedCursorStyle: u32 {
        /// The caret is drawn.
        const VISIBLE = 1;
        /// The caret is drawn as an underline.
        const SHAPE_UNDERLINE = 1 << 1;
        /// The caret is drawn as a bar.
        const SHAPE_BAR = 2 << 1;
        /// The caret is drawn as an outline rather than filled.
        const HOLLOW = 1 << 4;
    }
}

/// The inputs the paint policy reads beyond the projected caret.
#[derive(Debug, Clone, Copy)]
pub struct CaretPaintInput {
    /// Whether the host hides the caret for an IME composition.
    pub suppressed: bool,
    /// True when this pane is active and its window has focus.
    pub focused: bool,
    /// Whether an unfocused caret is drawn as a hollow block.
    pub unfocused_hollow: bool,
    /// Whether the blink phase is currently lit.
    pub phase_on: bool,
}

/// The stroke one frame draws the caret with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretStroke {
    /// A filled block covering the cell.
    Block,
    /// A line along the bottom of the cell.
    Underline,
    /// A vertical line at the left of the cell.
    Bar,
    /// A block outline, which an unfocused caret takes in place of its
    /// own shape.
    HollowBlock,
}

impl CaretStroke {
    /// Returns the stroke a caret is drawn with, or `None` when it is
    /// not painted this frame.
    ///
    /// `DECTCEM` and an IME composition each withhold the caret. An
    /// unfocused caret is painted regardless of the blink phase, as a
    /// hollow block unless `unfocused_hollow` is off. Only a caret the
    /// terminal asked to blink follows the phase.
    pub fn new(cursor: Cursor, input: CaretPaintInput) -> Option<Self> {
        if !cursor.visible || input.suppressed {
            return None;
        }
        if !input.focused && input.unfocused_hollow {
            return Some(Self::HollowBlock);
        }
        if !input.focused {
            return Some(Self::from(cursor.shape));
        }
        if cursor.blinking && !input.phase_on {
            return None;
        }
        Some(Self::from(cursor.shape))
    }
}

impl From<CaretStroke> for PackedCursorStyle {
    fn from(stroke: CaretStroke) -> Self {
        match stroke {
            CaretStroke::Block => Self::VISIBLE,
            CaretStroke::Underline => Self::VISIBLE | Self::SHAPE_UNDERLINE,
            CaretStroke::Bar => Self::VISIBLE | Self::SHAPE_BAR,
            CaretStroke::HollowBlock => Self::VISIBLE | Self::HOLLOW,
        }
    }
}

impl From<CursorShape> for CaretStroke {
    fn from(shape: CursorShape) -> Self {
        match shape {
            CursorShape::Block => Self::Block,
            CursorShape::Underline => Self::Underline,
            CursorShape::Bar => Self::Bar,
        }
    }
}

/// What one frame paints for the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretPaint {
    /// The viewport cell the caret occupies.
    pub pos: UVec2,
    /// The stroke it is drawn with.
    pub stroke: CaretStroke,
}

impl CaretPaint {
    /// Returns what to paint from the caret the view projected, or
    /// `None` when nothing projects into the viewport or the policy
    /// withholds the caret.
    pub fn new(caret: Option<(UVec2, Cursor)>, input: CaretPaintInput) -> Option<Self> {
        let (pos, cursor) = caret?;
        CaretStroke::new(cursor, input).map(|stroke| Self { pos, stroke })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(shape: CursorShape, blinking: bool, visible: bool) -> Cursor {
        Cursor {
            shape,
            blinking,
            visible,
            ..Default::default()
        }
    }

    fn at(shape: CursorShape, blinking: bool, visible: bool) -> Option<(UVec2, Cursor)> {
        Some((UVec2::new(3, 5), cursor(shape, blinking, visible)))
    }

    /// A focused caret in a lit blink phase, with hollow rendering on.
    fn lit() -> CaretPaintInput {
        CaretPaintInput {
            suppressed: false,
            focused: true,
            unfocused_hollow: true,
            phase_on: true,
        }
    }

    /// Asserts that a view with nothing projected paints nothing.
    ///
    /// Case: the user scrolls back through history until the shell's
    /// caret leaves the viewport.
    #[test]
    fn nothing_projected_paints_nothing() {
        assert_eq!(CaretPaint::new(None, lit()), None);
    }

    /// Asserts that a caret the terminal hid, and one the host
    /// suppresses, are both left unpainted.
    ///
    /// Case: vim hides the caret with `CSI ?25l` while it redraws, and
    /// separately an IME composition opens over the prompt.
    #[test]
    fn a_hidden_or_suppressed_caret_paints_nothing() {
        assert_eq!(
            CaretPaint::new(at(CursorShape::Block, false, false), lit()),
            None
        );
        let suppressed = CaretPaintInput {
            suppressed: true,
            ..lit()
        };
        assert_eq!(
            CaretPaint::new(at(CursorShape::Block, false, true), suppressed),
            None
        );
    }

    /// Asserts that an unfocused caret is painted as a hollow block and
    /// stays painted through the dark phase.
    ///
    /// Case: the user switches to another pane while a bar caret is
    /// blinking in this one.
    #[test]
    fn an_unfocused_caret_becomes_a_hollow_block_and_ignores_the_phase() {
        let unfocused = CaretPaintInput {
            focused: false,
            phase_on: false,
            ..lit()
        };
        let paint = CaretPaint::new(at(CursorShape::Bar, true, true), unfocused)
            .expect("the caret is painted");
        assert_eq!(paint.stroke, CaretStroke::HollowBlock);
        assert_eq!(paint.pos, UVec2::new(3, 5));
    }

    /// Asserts that an unfocused caret keeps its shape and stays
    /// painted through the dark phase when hollow rendering is off.
    ///
    /// Case: the user sets `unfocused_hollow = false` and switches
    /// panes while the caret is blinking.
    #[test]
    fn hollow_rendering_can_be_turned_off_without_resuming_the_blink() {
        let plain = CaretPaintInput {
            focused: false,
            unfocused_hollow: false,
            phase_on: false,
            ..lit()
        };
        let paint =
            CaretPaint::new(at(CursorShape::Bar, true, true), plain).expect("the caret is painted");
        assert_eq!(paint.stroke, CaretStroke::Bar);
    }

    /// Asserts that a blinking caret is dropped in the dark phase and
    /// painted in the lit one, while a steady caret ignores the phase.
    ///
    /// Case: the user has just typed and watches the caret blink.
    #[test]
    fn only_a_blinking_caret_follows_the_phase() {
        let dark = CaretPaintInput {
            phase_on: false,
            ..lit()
        };
        assert_eq!(
            CaretPaint::new(at(CursorShape::Block, true, true), dark),
            None
        );
        assert!(CaretPaint::new(at(CursorShape::Block, true, true), lit()).is_some());
        assert!(CaretPaint::new(at(CursorShape::Bar, false, true), dark).is_some());
    }

    /// Asserts that every caret shape occupies its assigned wire bits.
    ///
    /// Case: a terminal application switches among the block,
    /// underline, and bar DECSCUSR variants.
    #[test]
    fn each_shape_lands_in_the_shape_bits() {
        assert_eq!(PackedCursorStyle::from(CaretStroke::Block).bits(), 0b0001);
        assert_eq!(
            PackedCursorStyle::from(CaretStroke::Underline).bits(),
            0b0011
        );
        assert_eq!(PackedCursorStyle::from(CaretStroke::Bar).bits(), 0b0101);
    }

    /// Asserts that a hollow caret sets its own bit alongside the shape.
    ///
    /// Case: the caret sits in a pane the user is not typing into.
    #[test]
    fn a_hollow_caret_sets_its_bit() {
        assert_eq!(
            PackedCursorStyle::from(CaretStroke::HollowBlock).bits(),
            0b1_0001
        );
    }

    /// Asserts that a packed caret always marks itself visible, since a
    /// caret that is not painted has no packed form.
    ///
    /// Case: the shader decides whether to draw from this bit alone.
    #[test]
    fn the_packed_style_always_marks_the_caret_visible() {
        for stroke in [
            CaretStroke::Block,
            CaretStroke::Underline,
            CaretStroke::Bar,
            CaretStroke::HollowBlock,
        ] {
            assert!(PackedCursorStyle::from(stroke).contains(PackedCursorStyle::VISIBLE));
        }
    }
}
