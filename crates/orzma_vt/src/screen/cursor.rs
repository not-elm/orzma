//! Cursor state saved by DECSC and restored by DECRC.

use crate::schema::{GridColumn, ScreenLine};
use crate::screen::cell::Pen;
use crate::screen::character_sets::CharacterSetMapping;

/// Cursor state saved by DECSC, restored by DECRC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedCursor {
    /// Saved cursor row within the visible screen.
    pub line: ScreenLine,
    /// Saved cursor column.
    pub column: GridColumn,
    /// Saved SGR pen.
    pub pen: Pen,
    /// Saved deferred-wrap flag.
    pub pending_wrap: bool,
    /// Saved character set mapping.
    pub character_set_mapping: CharacterSetMapping,
}

/// Per-screen save slots for DECSC (the ANSI slot arrives later).
#[derive(Default)]
pub struct SavedCursorSlots {
    /// The DECSC slot; `None` until a save happens.
    pub dec: Option<SavedCursor>,
}
