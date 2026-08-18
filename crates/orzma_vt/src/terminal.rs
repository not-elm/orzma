//! Terminal state: the screen pair plus the modal state around it.
//!
//! [`TerminalState`] holds everything the terminal means — cell storage
//! and the write cursor per screen, the DECSET modes, tab stops, the
//! color table, and the title stack. It owns no emission state and
//! never decides when a frame goes out.
#![expect(
    dead_code,
    reason = "the executor and the frame emitter reach this state once they land"
)]

use crate::damage::Damage;
use crate::schema::{DisplayOffset, GridSize, Palette, Scroll, VtModes};
use crate::screen::Screen;

/// Everything the terminal means, independent of how it is emitted.
pub(crate) struct TerminalState {
    screens: Screens,
    modes: ModeState,
    tabs: TabStops,
    colors: ColorTable,
    title: TitleState,
}

impl TerminalState {
    /// Builds a blank terminal with the primary screen active.
    pub fn new(_size: GridSize, _max_history: usize) -> Self {
        todo!()
    }

    /// The screen the terminal currently reads and writes.
    pub fn active(&self) -> &Screen {
        todo!()
    }

    /// The screen the terminal currently reads and writes.
    pub fn active_mut(&mut self) -> &mut Screen {
        todo!()
    }

    /// Resizes both screens, reflowing content; `None` when the
    /// dimensions already matched.
    // TODO: Forward the `HistoryEvent::Reflowed` the reflow produces to
    // `PlacementStore` so anchors survive; the store is a sibling
    // field, so the caller has to route it.
    pub fn resize(&mut self, _size: GridSize) -> Option<Damage> {
        todo!()
    }

    /// Moves the active viewport; `None` for a clamped or zero motion.
    pub fn scroll(&mut self, _scroll: Scroll) -> Option<Damage> {
        todo!()
    }

    /// Grid dimensions of the active screen.
    pub fn grid_size(&self) -> GridSize {
        todo!()
    }

    /// Scrollback rows the active viewport sits above the live tail.
    pub fn display_offset(&self) -> DisplayOffset {
        todo!()
    }

    /// Snapshot of the input-relevant terminal modes.
    pub fn modes(&self) -> VtModes {
        todo!()
    }

    /// The live palette symbolic colors resolve against.
    pub fn palette(&self) -> Palette {
        todo!()
    }
}

/// The primary / alternate pair and which of the two is shown.
struct Screens {
    primary: Screen,
    alternate: Screen,
    active: ScreenKind,
}

/// Which screen of a [`Screens`] pair is shown.
enum ScreenKind {
    Primary,
    Alternate,
}

/// The DECSET flags plus the modes only the executor consults.
// TODO: Carry the DECSET group behind `VtModes` alongside the internal
// insert / origin / newline / autowrap modes.
struct ModeState {}

/// Horizontal tab stops.
// TODO: Carry the stop set plus HTS / TBC editing.
struct TabStops {}

/// The base palette and its dynamic overrides.
// TODO: Carry the base palette plus the OSC 4 / 10 / 11 / 12 overrides.
struct ColorTable {}

/// The current window title and its stack.
// TODO: Carry the current title plus the CSI 22 / 23 t stack.
struct TitleState {}
