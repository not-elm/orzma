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

impl TextCursorStyle {
    /// Returns the style pairing `shape` with `blink`.
    pub fn new(shape: CursorShape, blink: CursorBlink) -> Self {
        Self { shape, blink }
    }
}

/// The host-supplied policy the device applies to cursor sequences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CursorPolicy {
    /// The style `DECSCUSR` with a zero or a seven and `RIS` restore,
    /// and the style a terminal starts with. `DECSTR` does not.
    pub initial: TextCursorStyle,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the default policy is a steady block, which a host
    /// that injects a policy overrides.
    ///
    /// Case: a host builds a terminal without injecting a policy, as a
    /// test fixture does.
    #[test]
    fn the_default_policy_is_a_steady_block() {
        let policy = CursorPolicy::default();
        assert_eq!(policy.initial.shape, CursorShape::Block);
        assert_eq!(policy.initial.blink, CursorBlink::Steady);
    }
}
