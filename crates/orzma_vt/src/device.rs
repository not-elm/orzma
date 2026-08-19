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
use crate::schema::{DisplayOffset, GridSize, Palette, ScreenKind, Scroll, VtModes};
use crate::screen::Screen;

/// The emulated terminal device: screens, modes, tabs, colors, and
/// title.
///
/// It owns no parser, placement-extension, damage, or emission state —
/// those are the VT's own machinery and sit beside it in
/// [`crate::OrzmaVt`].
pub(crate) struct DeviceState {
    screens: Screens,
    // TODO: The modes only the executor consults — DECAWM, IRM, LNM,
    // and DECOM — do not belong here. Each lands beside the state it
    // governs, the way DECTCEM already lives in `Cursor::visible`:
    // wrapping and insert next to `pending_wrap` in `ScreenState`,
    // origin next to `Margins` in `Screen`.
    modes: VtModes,
    tabs: TabStops,
    colors: ColorTable,
    title: TitleState,
}

impl DeviceState {
    /// Builds a blank device with the primary screen active.
    ///
    /// The alternate screen is built without scrollback: a full-screen
    /// application has nothing to scroll back to, and its viewport stays
    /// pinned to the live tail.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            screens: Screens {
                primary: Screen::new(size, max_history),
                alternate: Screen::new(size, 0),
            },
            modes: VtModes::default(),
            tabs: TabStops {},
            colors: ColorTable {},
            title: TitleState {},
        }
    }

    /// The screen the device currently reads and writes.
    pub fn active(&self) -> &Screen {
        match self.modes.active_screen {
            ScreenKind::Primary => &self.screens.primary,
            ScreenKind::Alternate => &self.screens.alternate,
        }
    }

    /// The screen the device currently reads and writes.
    pub fn active_mut(&mut self) -> &mut Screen {
        match self.modes.active_screen {
            ScreenKind::Primary => &mut self.screens.primary,
            ScreenKind::Alternate => &mut self.screens.alternate,
        }
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
        self.active().grid_size()
    }

    /// Scrollback rows the active viewport sits above the live tail.
    pub fn display_offset(&self) -> DisplayOffset {
        self.active().display_offset()
    }

    /// Snapshot of the input-relevant device modes.
    pub fn modes(&self) -> VtModes {
        self.modes
    }

    /// The live palette symbolic colors resolve against.
    // TODO: Read the table from `ColorTable` once OSC 4 / 10 / 11 / 12
    // can override it.
    pub fn palette(&self) -> Palette {
        Palette::default()
    }
}

/// The primary / alternate pair.
///
/// Pure storage: which of the two is shown lives in
/// [`VtModes::active_screen`], so this struct cannot contradict it.
struct Screens {
    primary: Screen,
    alternate: Screen,
}

/// Horizontal tab stops.
// TODO: Carry the stop set plus HTS / TBC editing.
struct TabStops {}

/// The base palette and its dynamic overrides.
// TODO: Carry the base palette plus the OSC 4 / 10 / 11 / 12 overrides.
struct ColorTable {}

/// The current window title and its stack.
// TODO: Carry the current title plus the CSI 22 / 23 t stack.
struct TitleState {}
