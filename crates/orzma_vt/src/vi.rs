/// The direction of a vi-mode switch.
///
/// Named variants rather than a `bool` so the intent is readable at the
/// trigger site, where a bare `true` says nothing about which state it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViModeSwitch {
    /// Enter vi mode: the vi cursor starts tracking and keyboard input is
    /// interpreted as motions rather than forwarded to the PTY.
    Enter,
    /// Leave vi mode and snap the viewport back to the live tail.
    Exit,
}
