//! The character-terminal device this VT emulates.
//!
//! [`DeviceState`] is the device model, not a layer of its own: the
//! screens with their write cursors, the DECSET modes, the tab stops,
//! the color table, and the title stack. `OrzmaTerm` one crate up is
//! the live terminal — a VT wired to a PTY — so the device the VT
//! emulates deliberately does not borrow that name.
#![expect(
    dead_code,
    reason = "the executor and the frame emitter reach this state once they land"
)]

use crate::damage::Damage;
use crate::schema::{DisplayOffset, GridSize, Palette, Scroll, VtModes};
use crate::screen::Screen;

/// The emulated terminal device: screens, modes, tabs, colors, and
/// title.
///
/// It owns no parser, placement-extension, damage, or emission state —
/// those are the VT's own machinery and sit beside it in
/// [`crate::OrzmaVt`].
pub(crate) struct DeviceState {
    screens: Screens,
    modes: ModeState,
    tabs: TabStops,
    colors: ColorTable,
    title: TitleState,
}

impl DeviceState {
    /// Builds a blank device with the primary screen active.
    pub fn new(_size: GridSize, _max_history: usize) -> Self {
        todo!()
    }

    /// The screen the device currently reads and writes.
    pub fn active(&self) -> &Screen {
        todo!()
    }

    /// The screen the device currently reads and writes.
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

    /// Returns the grid dimensions of the active screen.
    pub fn grid_size(&self) -> GridSize {
        todo!()
    }

    /// Scrollback rows the active viewport sits above the live tail.
    pub fn display_offset(&self) -> DisplayOffset {
        todo!()
    }

    /// Snapshot of the input-relevant device modes.
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
