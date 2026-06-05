//! Neutral control events produced by the `Vt` engine (Bevy-free).
//!
//! The Bevy layer translates each `VtEvent` into the matching
//! `EntityEvent` (`TerminalBell` / `TerminalTitleChanged` / …).

use std::path::PathBuf;

/// A control event raised by the terminal. The Bevy layer translates
/// it into an `EntityEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtEvent {
    /// Bell (BEL).
    Bell,
    /// Window title change. `None` is an OSC 2 reset.
    TitleChanged(Option<String>),
    /// OSC 52 clipboard write.
    ClipboardStore(String),
    /// OSC 7 current-directory notification.
    CurrentDir(PathBuf),
    /// Terminal mode flags transitioned (DECSET/DECRST etc.).
    ModeChanged {
        /// Mode names that transitioned from unset to set.
        added: Vec<String>,
        /// Mode names that transitioned from set to unset.
        removed: Vec<String>,
    },
}
