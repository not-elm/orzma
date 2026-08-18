use crate::screen::cell::Pen;

/// Cursor state saved by DECSC, restored by DECRC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedCursor {
    /// Saved cursor row within the visible screen.
    pub line: u16,
    /// Saved cursor column.
    pub column: u16,
    /// Saved SGR pen.
    pub pen: Pen,
    /// Saved deferred-wrap flag.
    pub pending_wrap: bool,
}

/// Per-screen save slots for DECSC (the ANSI slot arrives later).
#[derive(Default)]
pub struct SavedCursorSlots {
    /// The DECSC slot; `None` until a save happens.
    pub dec: Option<SavedCursor>,
}
