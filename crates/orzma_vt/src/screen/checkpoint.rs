//! The write position and attributes DECSC copies aside for DECRC.

use crate::schema::{GridColumn, ScreenLine};
use crate::screen::cell::Pen;
use crate::screen::character_sets::CharacterSetMapping;

/// What `DECSC` copies aside so that `DECRC` can put it back.
///
/// This is a deliberate subset: it carries where the next character
/// lands and how it will be drawn, and nothing of what is already on
/// the screen. Restoring it never undoes a print.
///
/// # Control Functions
///
/// - `DECSC` (`ESC 7`)
/// - `DECRC` (`ESC 8`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
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
