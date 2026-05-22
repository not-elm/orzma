//! Plain `Action` enum produced by the dispatcher and consumed by
//! `multiplexer::commands::apply`. Kept as a synchronous plain value
//! (not a Bevy `Event`) so the dispatcher applies it in the same frame
//! and so it stays Bevy-independent for unit testing.

/// User-initiated mutation request triggered by a Ctrl-B prefix shortcut.
#[derive(Debug, Clone, Copy)]
pub enum Action {
    /// Create a new Window in the focused Session.
    NewWindow,
    /// Split the focused Pane top/bottom.
    SplitPaneHorizontal,
    /// Split the focused Pane left/right.
    SplitPaneVertical,
    /// Add a new Activity to the focused Pane and activate it.
    NewActivity,
}
