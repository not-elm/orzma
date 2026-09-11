//! The write position and attributes DECSC copies aside for DECRC.

use crate::screen::cell::Pen;
use crate::screen::character_sets::CharacterSetMapping;
use crate::screen::grid::coords::{GridColumn, ScreenLine};
use crate::screen::margins::OriginMode;
use crate::screen::state::ScreenState;

/// What `DECSC` copies aside so that `DECRC` can put it back.
///
/// Although a name such as `Save(d) Cursor` is used in the VT510 specification,
/// we use the name `Checkpoint` instead because the saved data actually includes information other than the cursor state.
///
/// The default value is what `DECRC` restores when no `DECSC` ever ran,
/// as far as this crate models that state: the home position, a reset
/// origin mode, no character attributes, and the default character set
/// mapping. The manual's fourth item also maps a set into GR, and its
/// separate selective erase attribute has no field here; both stay out
/// of scope for the reasons [`super::character_sets`] records. A
/// resize moves the saved row with the grid, so after one the
/// never-saved position may sit below home; [`super::Screen::resize`]
/// records why.
///
/// `IRM` is deliberately absent: it is not among the items `DECSC`
/// saves, and xterm masks the insert flag out of what `DECRC` restores,
/// so a later mutation that adds it here would make an alternate-screen
/// flip carry a mode no reference terminal carries.
///
/// `DECSTR` and `RIS` reset the saved state as well, and put back this
/// same default rather than leaving the last `DECSC` reachable.
///
/// # Control Functions
///
/// - `DECSC` (`ESC 7`)
/// - `DECRC` (`ESC 8`)
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
    /// The scroll margins are deliberately absent: `DECSC` saves the
    /// origin mode but not the region it is relative to, so a `DECRC`
    /// after a `DECSTBM` restores the mode against the newer margins.
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
