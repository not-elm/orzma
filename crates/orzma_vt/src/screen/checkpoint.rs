//! The write position and attributes DECSC copies aside for DECRC.

use crate::screen::cell::Pen;
use crate::screen::character_sets::CharacterSetMapping;
use crate::screen::grid::coords::{GridColumn, ScreenLine};
use crate::screen::margins::OriginMode;
use crate::screen::state::ScreenState;

/// What `DECSC` copies aside so that `DECRC` can put it back.
///
/// The default value is what `DECRC` restores when no `DECSC` ever ran,
/// as far as this crate models that state: the home position, a reset
/// origin mode, no character attributes, and the default character set
/// mapping. The VT510 manual's fourth item also maps a set into GR, and its
/// separate selective erase attribute has no field here. A resize moves
/// the saved row with the grid, so after one the never-saved position
/// may sit below home.
///
/// `DECRC` restores neither `IRM` nor `DECTCEM`.
///
/// The `Wrap flag (autowrap or no autowrap)` the VT420 and VT520
/// manuals list among the items `DECSC` saves is the last-column flag
/// `Self::pending_wrap` holds, not `DECAWM`: DEC STD-070 p.D-14 has the
/// flag "saved when a Save Cursor operation is performed, and restored
/// when a Restore Cursor operation is performed". `DECAWM` is not saved.
///
/// `RIS` resets the saved state as well, and puts back this same default
/// rather than leaving the last `DECSC` reachable.
///
/// # Control Functions
///
/// - `DECSC` (`ESC 7`)
/// - `DECRC` (`ESC 8`)
/// - `SCOSC` (`CSI s`)
/// - `SCORC` (`CSI u`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

impl Checkpoint {
    /// Reads off everything `DECSC` saves from the state that holds it.
    ///
    /// The scroll margins are not saved, so a `DECRC` after a `DECSTBM`
    /// restores the origin mode against the newer margins.
    pub(super) fn capture(
        state: &ScreenState,
        origin_mode: OriginMode,
        character_set_mapping: CharacterSetMapping,
    ) -> Self {
        Self {
            line: state.line,
            column: state.column,
            pen: state.pen,
            pending_wrap: state.pending_wrap,
            origin_mode,
            character_set_mapping,
        }
    }
}
