//! The character-terminal device this VT emulates.
//!
//! [`DeviceState`] is the device model, not a layer of its own: the
//! screens with their write cursors, the DECSET modes, the tab stops,
//! the color table, and the title stack. `OrzmaTty` one crate up is
//! the live terminal — a VT wired to a PTY — so the device the VT
//! emulates deliberately does not borrow that name.
#![expect(
    dead_code,
    reason = "the executor and the frame emitter reach this state once they land"
)]

use crate::damage::Damage;
use crate::schema::{DisplayOffset, GridColumn, GridSize, Palette, ScreenKind, Scroll, VtModes};
use crate::screen::Screen;
use crate::screen::grid::LineId;

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

    /// The screen the device reads and writes, paired with its kind.
    pub fn active_screen(&self) -> ActiveScreen<'_> {
        ActiveScreen {
            kind: self.modes.active_screen,
            screen: self.active(),
        }
    }

    /// Resizes both screens, reflowing content; `None` when the
    /// dimensions already matched.
    // TODO: A rewrap can insert or drop rows in the middle of the ring,
    // and can split or merge them, so a surviving placement anchor has to
    // be told which resulting row it now belongs to. Once reflow lands,
    // route the row remapping it produces to `PlacementStore` (a sibling
    // field, so the caller has to route it) so it can re-anchor each
    // placement to its surviving row.
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

    /// Switches the active screen.
    ///
    /// The production path is the `?1049` handler, which is not
    /// implemented yet; this exists so the placement tests can reach the
    /// alternate screen.
    #[cfg(test)]
    pub(crate) fn set_active_screen_for_test(&mut self, kind: ScreenKind) {
        self.modes.active_screen = kind;
    }
}

/// The active screen together with which of the two it is.
///
/// [`crate::screen::grid::LineId`] is unique per grid, so an anchor
/// resolved against the other screen's grid silently names a different
/// row. Pairing the two makes that mismatch unconstructible.
#[derive(Clone, Copy)]
pub(crate) struct ActiveScreen<'a> {
    kind: ScreenKind,
    screen: &'a Screen,
}

impl ActiveScreen<'_> {
    /// Which of the two screens this is.
    pub fn kind(&self) -> ScreenKind {
        self.kind
    }

    /// The id of the row the cursor sits on.
    pub fn cursor_line_id(&self) -> LineId {
        self.screen.cursor_line_id()
    }

    /// The signed viewport row `id` now sits at; `None` once the row has
    /// left the ring.
    pub fn viewport_row_of(&self, id: LineId) -> Option<i32> {
        self.screen.viewport_row_of(id)
    }

    /// The cursor's column.
    pub fn cursor_column(&self) -> GridColumn {
        self.screen.cursor_column()
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

/// The base palette and its dynamic overrides.
// TODO: Carry the base palette plus the OSC 4 / 10 / 11 / 12 overrides.
struct ColorTable {}

/// The current window title and its stack.
// TODO: Carry the current title plus the CSI 22 / 23 t stack.
struct TitleState {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a stop set on one screen is absent from the other.
    ///
    /// The agreed policy gives each screen its own table, so a
    /// full-screen application cannot disturb the tab positions the
    /// shell left on the primary screen. xterm, VTE, alacritty,
    /// wezterm, ghostty, and Windows Terminal share one table across
    /// both screens instead; ECMA-48 settles nothing here, because it
    /// has no alternate screen at all.
    ///
    /// Case: a shell installs its own tab positions, then a full-screen
    /// editor takes over the alternate screen and emits a tab.
    #[test]
    fn the_two_screens_carry_independent_tab_stops() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device.active_mut().print(c);
        }
        device.active_mut().set_horizontal_tab_stop();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.active_mut().move_forward_tabs(1);
        assert_eq!(device.active().cursor_column(), GridColumn(8));

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.active_mut().carriage_return();
        device.active_mut().move_forward_tabs(1);
        assert_eq!(device.active().cursor_column(), GridColumn(3));
    }

    /// Asserts that a checkpoint saved on one screen is unreachable from
    /// the other.
    ///
    /// The agreed policy gives each screen its own checkpoint, so a
    /// restore on the alternate screen returns its own power-up state
    /// instead of consuming the save the shell left on the primary.
    /// VT510 documents a separate `DECSC` buffer only for the main
    /// display and the status line, so the alternate screen is this
    /// terminal's own decision.
    ///
    /// Case: a shell saves its cursor, a full-screen editor takes over
    /// the alternate screen and emits a restore of its own, and the
    /// shell then restores after the editor exits.
    #[test]
    fn the_two_screens_carry_independent_checkpoints() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device.active_mut().print(c);
        }
        device.active_mut().save_checkpoint();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.active_mut().print('x');
        device.active_mut().restore_checkpoint();
        assert_eq!(device.active().cursor_column(), GridColumn(0));

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.active_mut().carriage_return();
        device.active_mut().restore_checkpoint();
        assert_eq!(device.active().cursor_column(), GridColumn(3));
    }
}
