//! The startup cursor policy a host injects into the device.

use crate::device::modes::{CursorBlink, CursorShape};

/// A cursor shape paired with its blink.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextCursorStyle {
    /// The shape the caret is drawn with.
    pub shape: CursorShape,
    /// Whether the caret blinks.
    pub blink: CursorBlink,
}

/// The host-supplied policy the device applies to cursor sequences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CursorPolicy {
    /// The style `DECSCUSR` with a zero or a seven, `RIS` and `DECSTR`
    /// restore, and the style a terminal starts with.
    pub initial: TextCursorStyle,
    /// When true, `DECSET 12` and `DECRST 12` leave the blink untouched.
    pub ignore_dec_mode_12: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the default policy is a steady block that honors DEC
    /// mode 12, matching the power-up state.
    ///
    /// Case: a host that injects no policy at all, such as a test fixture.
    #[test]
    fn the_default_policy_is_a_steady_block_that_honors_mode_twelve() {
        let policy = CursorPolicy::default();
        assert_eq!(policy.initial.shape, CursorShape::Block);
        assert_eq!(policy.initial.blink, CursorBlink::Steady);
        assert!(!policy.ignore_dec_mode_12);
    }
}
