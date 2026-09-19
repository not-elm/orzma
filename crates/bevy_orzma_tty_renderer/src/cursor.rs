//! Cursor paint policy: focus, hollow rendering, and the blink phase.

use bevy::prelude::*;
use orzma_vt::prelude::{Cursor, CursorShape};
use std::time::Duration;

/// Bit 0 of the packed `cursor_style` u32 — set when the caret is drawn.
pub const CURSOR_VISIBLE_BIT: u32 = 1;

/// Bits 1-2 of the packed `cursor_style` u32, carrying the shape
/// (Block `0`, Underline `1`, Bar `2`).
pub const CURSOR_SHAPE_MASK: u32 = 0b110;

/// Bit 3 of the packed `cursor_style` u32 — set when the caret blinks.
pub const CURSOR_BLINKING_BIT: u32 = 8;

/// Bit 4 of the packed `cursor_style` u32 — set when the caret is drawn
/// as an outline rather than filled.
pub const CURSOR_HOLLOW_BIT: u32 = 16;

/// Packs `cursor` into the `cursor_style` u32 the shader decodes:
/// [`CURSOR_VISIBLE_BIT`], [`CURSOR_SHAPE_MASK`] carrying the shape
/// (Block `0`, Underline `1`, Bar `2`), and [`CURSOR_BLINKING_BIT`].
///
/// [`CURSOR_HOLLOW_BIT`] is left clear; [`CursorPaint::resolve`] sets it.
pub fn pack_cursor_style(cursor: &Cursor) -> u32 {
    let visible = if cursor.visible {
        CURSOR_VISIBLE_BIT
    } else {
        0
    };
    let shape = match cursor.shape {
        CursorShape::Block => 0u32,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
    };
    let blinking = if cursor.blinking {
        CURSOR_BLINKING_BIT
    } else {
        0
    };
    visible | (shape << 1) | blinking
}

/// The inputs the paint policy reads beyond the packed style.
#[derive(Debug, Clone, Copy)]
pub struct CursorPaintInput {
    /// True when this pane is active and its window has focus.
    pub focused: bool,
    /// Whether an unfocused caret is drawn as a hollow block.
    pub unfocused_hollow: bool,
    /// Whether the blink phase is currently lit.
    pub phase_on: bool,
}

/// The packed cursor style one frame uploads to the shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPaint(u32);

impl CursorPaint {
    /// Resolves the style the shader receives from the style the view
    /// already packed.
    ///
    /// `packed` carries the visibility the earlier stages decided:
    /// `DECTCEM`, IME preedit, a cursor scrolled out of the viewport,
    /// and the vi cursor's own forced visibility all reach this through
    /// [`CURSOR_VISIBLE_BIT`].
    ///
    /// # Invariants
    ///
    /// - A cleared [`CURSOR_VISIBLE_BIT`] is never set again.
    /// - Every path that does not follow the blink phase leaves
    ///   [`CURSOR_VISIBLE_BIT`] as it found it.
    pub fn resolve(packed: u32, input: CursorPaintInput) -> Self {
        if packed & CURSOR_VISIBLE_BIT == 0 {
            return Self(packed);
        }
        if !input.focused {
            if input.unfocused_hollow {
                return Self((packed & !CURSOR_SHAPE_MASK) | CURSOR_HOLLOW_BIT);
            }
            return Self(packed);
        }
        if packed & CURSOR_BLINKING_BIT != 0 && !input.phase_on {
            return Self(packed & !CURSOR_VISIBLE_BIT);
        }
        Self(packed)
    }

    /// The packed style, ready for the uniform.
    pub fn packed(self) -> u32 {
        self.0
    }
}

/// Whether the blink phase is lit `elapsed` after the last keystroke.
///
/// Reports `true` once `timeout` has passed, and for a zero `interval`.
/// A `None` timeout blinks indefinitely.
pub fn blink_phase_on(elapsed: Duration, interval: Duration, timeout: Option<Duration>) -> bool {
    if let Some(timeout) = timeout
        && elapsed >= timeout
    {
        return true;
    }
    let interval_ms = interval.as_millis();
    if interval_ms == 0 {
        return true;
    }
    (elapsed.as_millis() / interval_ms).is_multiple_of(2)
}

/// The cursor drawing settings the renderer reads each frame.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CursorRenderConfig {
    /// The interval between blink phases.
    pub blink_interval: Duration,
    /// How long the caret keeps blinking with no keystroke; `None`
    /// blinks indefinitely.
    pub blink_timeout: Option<Duration>,
    /// Caret thickness as a fraction of the cell width.
    pub thickness: f32,
    /// Whether an unfocused caret is drawn as a hollow block.
    pub unfocused_hollow: bool,
}

impl Default for CursorRenderConfig {
    fn default() -> Self {
        Self {
            blink_interval: Duration::from_millis(750),
            blink_timeout: Some(Duration::from_secs(5)),
            thickness: 0.15,
            unfocused_hollow: true,
        }
    }
}

/// The real-time elapsed reading at the last keystroke, which the blink
/// phase counts from. A terminal that has seen no keystroke counts from
/// startup.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LastKeyInstant(pub Duration);

/// Registers the resources the cursor paint policy reads.
pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CursorRenderConfig>()
            .init_resource::<LastKeyInstant>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::{GridColumn, GridLine, GridPoint};

    fn cursor(shape: CursorShape, blinking: bool, visible: bool) -> Cursor {
        Cursor {
            point: GridPoint::default(),
            shape,
            blinking,
            visible,
        }
    }

    fn packed(shape: CursorShape, blinking: bool) -> u32 {
        pack_cursor_style(&cursor(shape, blinking, true))
    }

    /// Asserts that every cursor shape occupies its assigned wire bits.
    ///
    /// Case: a terminal application switches among the steady block,
    /// underline, and bar DECSCUSR variants.
    #[test]
    fn each_shape_lands_in_the_shape_bits() {
        assert_eq!(packed(CursorShape::Block, false), 0b0001);
        assert_eq!(packed(CursorShape::Underline, false), 0b0011);
        assert_eq!(packed(CursorShape::Bar, false), 0b0101);
    }

    /// Asserts that blinking is encoded independently for every shape.
    ///
    /// Case: a terminal application selects a blinking caret variant
    /// while the terminal stays focused.
    #[test]
    fn blinking_sets_its_bit_independent_of_shape() {
        assert_eq!(packed(CursorShape::Block, true), 0b1001);
        assert_eq!(packed(CursorShape::Underline, true), 0b1011);
        assert_eq!(packed(CursorShape::Bar, true), 0b1101);
    }

    /// Asserts that a hidden cursor clears only the visible bit,
    /// leaving the packed shape and blink policy intact.
    ///
    /// Case: vim hides the cursor with DECTCEM (`CSI ?25l`) while it
    /// redraws, and on `CSI ?25h` the caret returns.
    #[test]
    fn a_hidden_cursor_clears_only_the_visible_bit() {
        assert_eq!(
            pack_cursor_style(&cursor(CursorShape::Bar, true, false)),
            0b1100
        );
        assert_eq!(
            pack_cursor_style(&cursor(CursorShape::Block, false, false)),
            0b0000
        );
        assert_eq!(
            pack_cursor_style(&cursor(CursorShape::Bar, true, false)),
            pack_cursor_style(&cursor(CursorShape::Bar, true, true)) & !CURSOR_VISIBLE_BIT
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
            pack_cursor_style(&at_origin),
            pack_cursor_style(&deep_in_history)
        );
    }

    fn block_blinking() -> u32 {
        packed(CursorShape::Block, true)
    }

    fn bar_blinking() -> u32 {
        packed(CursorShape::Bar, true)
    }

    fn bar_steady() -> u32 {
        packed(CursorShape::Bar, false)
    }

    fn focused(phase_on: bool) -> CursorPaintInput {
        CursorPaintInput {
            focused: true,
            unfocused_hollow: true,
            phase_on,
        }
    }

    fn unfocused(unfocused_hollow: bool) -> CursorPaintInput {
        CursorPaintInput {
            focused: false,
            unfocused_hollow,
            phase_on: false,
        }
    }

    /// Asserts that a cursor the earlier stages already hid stays
    /// hidden, and is not turned into a hollow block by being
    /// unfocused.
    ///
    /// Case: vim hides the caret with `CSI ?25l` while it repaints, and
    /// the user clicks another pane during the repaint.
    #[test]
    fn an_already_hidden_cursor_stays_hidden_when_unfocused() {
        let hidden = block_blinking() & !CURSOR_VISIBLE_BIT;
        let paint = CursorPaint::resolve(hidden, unfocused(true));
        assert_eq!(paint.packed() & CURSOR_VISIBLE_BIT, 0);
        assert_eq!(paint.packed() & CURSOR_HOLLOW_BIT, 0);
    }

    /// Asserts that an unfocused blinking cursor is drawn as a hollow
    /// block with its shape cleared, and stays visible through the dark
    /// phase.
    ///
    /// Case: the user switches to another pane while a bar caret is
    /// blinking in this one.
    #[test]
    fn an_unfocused_cursor_becomes_a_hollow_block_and_stops_blinking() {
        let paint = CursorPaint::resolve(bar_blinking(), unfocused(true));
        assert_eq!(paint.packed() & CURSOR_VISIBLE_BIT, CURSOR_VISIBLE_BIT);
        assert_eq!(paint.packed() & CURSOR_HOLLOW_BIT, CURSOR_HOLLOW_BIT);
        assert_eq!(paint.packed() & CURSOR_SHAPE_MASK, 0);
    }

    /// Asserts that an unfocused cursor keeps its shape and stays
    /// visible through the dark phase when hollow rendering is off.
    ///
    /// Case: the user sets `unfocused_hollow = false` and switches
    /// panes while the caret is blinking.
    #[test]
    fn hollow_rendering_can_be_turned_off_without_resuming_the_blink() {
        let paint = CursorPaint::resolve(bar_blinking(), unfocused(false));
        assert_eq!(paint.packed() & CURSOR_HOLLOW_BIT, 0);
        assert_eq!(
            paint.packed() & CURSOR_SHAPE_MASK,
            bar_blinking() & CURSOR_SHAPE_MASK
        );
        assert_eq!(paint.packed() & CURSOR_VISIBLE_BIT, CURSOR_VISIBLE_BIT);
    }

    /// Asserts that a blinking cursor is hidden in the dark phase and
    /// shown in the lit one, while a steady cursor ignores the phase.
    ///
    /// Case: the user has just typed and watches the caret blink.
    #[test]
    fn only_a_blinking_cursor_follows_the_phase() {
        assert_eq!(
            CursorPaint::resolve(block_blinking(), focused(false)).packed() & CURSOR_VISIBLE_BIT,
            0
        );
        assert_eq!(
            CursorPaint::resolve(block_blinking(), focused(true)).packed() & CURSOR_VISIBLE_BIT,
            CURSOR_VISIBLE_BIT
        );
        assert_eq!(
            CursorPaint::resolve(bar_steady(), focused(false)).packed() & CURSOR_VISIBLE_BIT,
            CURSOR_VISIBLE_BIT
        );
    }

    /// Asserts that the phase alternates on the interval and settles lit
    /// once the timeout passes, and that only a keystroke restarts it.
    ///
    /// Case: the user types, watches the caret blink, then leaves the
    /// keyboard alone while a program changes the cursor style and the
    /// window regains focus.
    #[test]
    fn the_phase_alternates_then_settles_lit_until_the_next_keystroke() {
        let interval = Duration::from_millis(750);
        let timeout = Some(Duration::from_secs(5));
        assert!(blink_phase_on(Duration::ZERO, interval, timeout));
        assert!(!blink_phase_on(
            Duration::from_millis(750),
            interval,
            timeout
        ));
        assert!(blink_phase_on(
            Duration::from_millis(1500),
            interval,
            timeout
        ));
        assert!(blink_phase_on(Duration::from_secs(5), interval, timeout));
        assert!(blink_phase_on(Duration::from_secs(600), interval, timeout));
    }

    /// Asserts that a `None` timeout blinks indefinitely rather than
    /// settling.
    ///
    /// Case: the user sets `blink_timeout = 0` and walks away.
    #[test]
    fn a_none_timeout_keeps_blinking() {
        let interval = Duration::from_millis(750);
        assert!(!blink_phase_on(Duration::from_millis(750), interval, None));
        assert!(!blink_phase_on(
            Duration::from_secs(6000) + Duration::from_millis(750),
            interval,
            None
        ));
    }

    /// Asserts that a zero interval does not divide by zero, reporting a
    /// lit phase instead.
    ///
    /// Case: a caller passes an unclamped interval straight from a
    /// malformed config.
    #[test]
    fn a_zero_interval_reports_a_lit_phase() {
        assert!(blink_phase_on(Duration::from_secs(1), Duration::ZERO, None));
    }
}
