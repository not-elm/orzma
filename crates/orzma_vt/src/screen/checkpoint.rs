//! The write position and attributes DECSC copies aside for DECRC.

use crate::schema::{GridColumn, ScreenLine};
use crate::screen::cell::Pen;
use crate::screen::character_sets::CharacterSetMapping;
use crate::screen::margins::OriginMode;

/// What `DECSC` copies aside so that `DECRC` can put it back.
///
/// Although a name such as `Save(d) Cursor` is used in the VT510 specification,
/// we use the name `Checkpoint` instead because the saved data actually includes information other than the cursor state.
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
    /// Saved cursor origin (`DECOM`).
    pub origin_mode: OriginMode,
    /// Saved character set mapping.
    pub character_set_mapping: CharacterSetMapping,
}
