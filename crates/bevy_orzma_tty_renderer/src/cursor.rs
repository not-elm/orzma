//! Caret paint policy: focus, hollow rendering, and the blink phase.

use bevy::prelude::*;
use orzma_vt::prelude::{Cursor, CursorShape};
use std::time::Duration;

/// Bit 0 of the packed `cursor_style` u32 — set when the caret is drawn.
pub const CURSOR_VISIBLE_BIT: u32 = 1;

/// Bit 4 of the packed `cursor_style` u32 — set when the caret is drawn
/// as an outline rather than filled.
pub const CURSOR_HOLLOW_BIT: u32 = 16;

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

/// What one frame paints for the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretPaint {
    /// The viewport cell the caret occupies.
    pub pos: UVec2,
    /// The shape it is drawn with.
    pub shape: CursorShape,
    /// Whether it is drawn as an outline rather than filled.
    pub hollow: bool,
}

impl CaretPaint {
    /// Resolves what to paint from the caret the view projected, or
    /// `None` when nothing is painted this frame.
    ///
    /// A caret is not painted when nothing projects into the viewport,
    /// when `DECTCEM` hides it, or while an IME composition suppresses
    /// it.
    ///
    /// # Invariants
    ///
    /// - An unfocused caret is painted regardless of the blink phase.
    /// - Only a caret the terminal asked to blink follows the phase.
    pub fn resolve(caret: Option<(UVec2, Cursor)>, input: CaretPaintInput) -> Option<Self> {
        let (pos, cursor) = caret?;
        if !cursor.visible || input.suppressed {
            return None;
        }
        if !input.focused {
            if input.unfocused_hollow {
                return Some(Self {
                    pos,
                    shape: CursorShape::Block,
                    hollow: true,
                });
            }
            return Some(Self {
                pos,
                shape: cursor.shape,
                hollow: false,
            });
        }
        if cursor.blinking && !input.phase_on {
            return None;
        }
        Some(Self {
            pos,
            shape: cursor.shape,
            hollow: false,
        })
    }

    /// The `cursor_style` u32 the shader decodes: [`CURSOR_VISIBLE_BIT`],
    /// bits 1-2 carrying the shape (Block `0`, Underline `1`, Bar `2`),
    /// and [`CURSOR_HOLLOW_BIT`].
    pub fn to_packed(self) -> u32 {
        let shape = match self.shape {
            CursorShape::Block => 0u32,
            CursorShape::Underline => 1,
            CursorShape::Bar => 2,
        };
        let hollow = if self.hollow { CURSOR_HOLLOW_BIT } else { 0 };
        CURSOR_VISIBLE_BIT | (shape << 1) | hollow
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

/// The caret drawing knobs the renderer reads each frame.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CaretStyle {
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

impl Default for CaretStyle {
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
        app.init_resource::<CaretStyle>()
            .init_resource::<LastKeyInstant>();
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

    fn input(focused: bool, unfocused_hollow: bool, phase_on: bool) -> CaretPaintInput {
        CaretPaintInput {
            suppressed: false,
            focused,
            unfocused_hollow,
            phase_on,
        }
    }

    /// Asserts that a view with nothing projected paints nothing.
    ///
    /// Case: the user scrolls back through history until the shell's
    /// caret leaves the viewport.
    #[test]
    fn nothing_projected_paints_nothing() {
        assert_eq!(CaretPaint::resolve(None, input(true, true, true)), None);
    }

    /// Asserts that a caret the terminal hid, and one the host
    /// suppresses, are both left unpainted.
    ///
    /// Case: vim hides the caret with `CSI ?25l` while it redraws, and
    /// separately an IME composition opens over the prompt.
    #[test]
    fn a_hidden_or_suppressed_caret_paints_nothing() {
        assert_eq!(
            CaretPaint::resolve(
                at(CursorShape::Block, false, false),
                input(true, true, true)
            ),
            None
        );
        let suppressed = CaretPaintInput {
            suppressed: true,
            ..input(true, true, true)
        };
        assert_eq!(
            CaretPaint::resolve(at(CursorShape::Block, false, true), suppressed),
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
        let paint =
            CaretPaint::resolve(at(CursorShape::Bar, true, true), input(false, true, false))
                .expect("the caret is painted");
        assert_eq!(paint.shape, CursorShape::Block);
        assert!(paint.hollow);
        assert_eq!(paint.pos, UVec2::new(3, 5));
    }

    /// Asserts that an unfocused caret keeps its shape and stays
    /// painted through the dark phase when hollow rendering is off.
    ///
    /// Case: the user sets `unfocused_hollow = false` and switches
    /// panes while the caret is blinking.
    #[test]
    fn hollow_rendering_can_be_turned_off_without_resuming_the_blink() {
        let paint =
            CaretPaint::resolve(at(CursorShape::Bar, true, true), input(false, false, false))
                .expect("the caret is painted");
        assert_eq!(paint.shape, CursorShape::Bar);
        assert!(!paint.hollow);
    }

    /// Asserts that a blinking caret is dropped in the dark phase and
    /// painted in the lit one, while a steady caret ignores the phase.
    ///
    /// Case: the user has just typed and watches the caret blink.
    #[test]
    fn only_a_blinking_caret_follows_the_phase() {
        assert_eq!(
            CaretPaint::resolve(at(CursorShape::Block, true, true), input(true, true, false)),
            None
        );
        assert!(
            CaretPaint::resolve(at(CursorShape::Block, true, true), input(true, true, true))
                .is_some()
        );
        assert!(
            CaretPaint::resolve(at(CursorShape::Bar, false, true), input(true, true, false))
                .is_some()
        );
    }

    /// Asserts that every caret shape occupies its assigned wire bits.
    ///
    /// Case: a terminal application switches among the block,
    /// underline, and bar DECSCUSR variants.
    #[test]
    fn each_shape_lands_in_the_shape_bits() {
        let packed = |shape| {
            CaretPaint {
                pos: UVec2::ZERO,
                shape,
                hollow: false,
            }
            .to_packed()
        };
        assert_eq!(packed(CursorShape::Block), 0b0001);
        assert_eq!(packed(CursorShape::Underline), 0b0011);
        assert_eq!(packed(CursorShape::Bar), 0b0101);
    }

    /// Asserts that a hollow caret sets its own bit alongside the shape.
    ///
    /// Case: the caret sits in a pane the user is not typing into.
    #[test]
    fn a_hollow_caret_sets_its_bit() {
        let paint = CaretPaint {
            pos: UVec2::ZERO,
            shape: CursorShape::Block,
            hollow: true,
        };
        assert_eq!(paint.to_packed(), CURSOR_VISIBLE_BIT | CURSOR_HOLLOW_BIT);
    }

    /// Asserts that a packed caret always marks itself visible, since a
    /// caret that is not painted has no packed form.
    ///
    /// Case: the shader decides whether to draw from this bit alone.
    #[test]
    fn the_packed_style_always_marks_the_caret_visible() {
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
            for hollow in [false, true] {
                let packed = CaretPaint {
                    pos: UVec2::ZERO,
                    shape,
                    hollow,
                }
                .to_packed();
                assert_eq!(packed & CURSOR_VISIBLE_BIT, CURSOR_VISIBLE_BIT);
            }
        }
    }

    /// Asserts that the phase alternates on the interval and settles
    /// lit once the timeout passes.
    ///
    /// Case: the user types, watches the caret blink, then leaves the
    /// keyboard alone.
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
