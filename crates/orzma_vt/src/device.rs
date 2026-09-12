//! The character-terminal device this VT emulates.

pub(crate) mod color;
pub(crate) mod modes;

use crate::device::color::{Palette, Rgb};
use crate::device::modes::{AutoWrap, ScreenKind, VtModes};
use crate::frame::damage::DamageSpan;
use crate::placement::{InstanceId, MAX_PLACEMENTS, PlacementSize};
use crate::screen::Screen;
use crate::screen::cursor::Cursor;
use crate::screen::grid::GridSize;
use crate::screen::grid::coords::{GridColumn, ScreenLine};
use crate::screen::viewport::{DisplayOffset, Scroll};
use std::collections::VecDeque;

/// The emulated terminal device: screens, modes, tabs, colors, title,
/// and the terminal-scoped placement invariants.
pub(crate) struct DeviceState {
    screens: Screens,
    modes: VtModes,
    palette: Palette,
    title: TitleState,
}

impl DeviceState {
    /// Builds a blank device with the primary screen active.
    ///
    /// The alternate screen is built without scrollback, so its viewport
    /// stays pinned to the live tail.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self::assert_nonzero_size(size);
        Self {
            screens: Screens {
                primary: Screen::new(size, max_history),
                alternate: Screen::new(size, 0),
            },
            modes: VtModes::default(),
            palette: Palette::default(),
            title: TitleState::default(),
        }
    }

    /// The screen the device currently reads and writes.
    pub fn active_screen(&self) -> &Screen {
        match self.modes.active_screen {
            ScreenKind::Primary => &self.screens.primary,
            ScreenKind::Alternate => &self.screens.alternate,
        }
    }

    /// The screen the device currently reads and writes.
    pub fn active_screen_mut(&mut self) -> &mut Screen {
        match self.modes.active_screen {
            ScreenKind::Primary => &mut self.screens.primary,
            ScreenKind::Alternate => &mut self.screens.alternate,
        }
    }

    /// Resizes both screens, truncating rather than reflowing; `None`
    /// when the dimensions already matched.
    ///
    /// The caller must reject a size with a zero axis before it reaches
    /// this method.
    ///
    /// Placements this strands are not named here: their anchors stop
    /// resolving, and the next [`Self::evict_lost_anchors`] names them.
    ///
    /// # Invariants
    ///
    /// A resize that changes the dimensions reports [`DamageSpan::Full`].
    ///
    /// Both grid axes are nonzero, and both screens are always the same
    /// size.
    pub fn resize(&mut self, size: GridSize) -> Option<DamageSpan> {
        Self::assert_nonzero_size(size);
        let primary = self.screens.primary.resize(size);
        let _ = self.screens.alternate.resize(size);
        primary
    }

    /// Rejects a degenerate grid size in debug builds.
    fn assert_nonzero_size(size: GridSize) {
        debug_assert!(
            size.cols > 0 && size.rows > 0,
            "a degenerate grid size is rejected before it reaches the device"
        );
    }

    /// Moves the active viewport; `None` for a clamped or zero motion.
    ///
    /// # Invariants
    ///
    /// A motion that moves the viewport reports [`DamageSpan::Full`].
    pub fn scroll(&mut self, scroll: Scroll) -> Option<DamageSpan> {
        self.active_screen_mut().scroll(scroll)
    }

    /// Returns both screens and every mode to their power-up state;
    /// `None` when the frame that follows needs no repaint.
    ///
    /// Only the screen left active by the mode reset reaches a frame, so
    /// the alternate screen's damage is dropped rather than folded in.
    /// A reset that arrives while the alternate screen is shown always
    /// repaints.
    ///
    /// The title is cleared too, dropping both the current title and the
    /// whole save stack.
    ///
    /// The whole palette returns to its built-in defaults too, and a
    /// reset that changes a color reports a full repaint even when
    /// neither screen was written.
    ///
    /// # Control Functions
    ///
    /// - `RIS` (`ESC c`)
    pub fn reset(&mut self) -> Option<DamageSpan> {
        let was_showing_alternate = matches!(self.modes.active_screen, ScreenKind::Alternate);
        let primary = self.screens.primary.reset();
        let _ = self.screens.alternate.reset();
        // NOTE: This wholesale write is the one place `auto_wrap` is set
        // without `DeviceState::set_auto_wrap`, and it is sound only because
        // the two screen resets above already cleared each screen's
        // live and saved deferred wrap. A partial mode reset such as
        // `DECSTR`, which leaves the screens alone, must go through
        // `set_auto_wrap` instead.
        self.modes = VtModes::default();
        self.title = TitleState::default();
        let palette_changed = self.palette.reset();
        (was_showing_alternate || primary.is_some() || palette_changed).then_some(DamageSpan::Full)
    }

    /// The window title the application last set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.current.as_deref()
    }

    /// Replaces the window title; `None` returns it to the host's
    /// default.
    ///
    /// # Control Functions
    ///
    /// - `OSC 0` / `OSC 2`
    pub fn set_title(&mut self, title: Option<String>) {
        self.title.current = title;
    }

    /// Saves the current title on the stack, dropping the oldest entry
    /// once the stack is full.
    ///
    /// # Control Functions
    ///
    /// - `XTWINOPS` (`CSI 22 t`); the icon/window selector and the
    ///   direct slot number xterm accepts after it are both ignored, so
    ///   a slot store arrives here as an ordinary push.
    pub fn push_title(&mut self) {
        if self.title.stack.len() == MAX_TITLE_DEPTH {
            self.title.stack.pop_front();
        }
        self.title.stack.push_back(self.title.current.clone());
    }

    /// Takes the most recently saved title off the stack without
    /// applying it; `None` when the stack was empty.
    ///
    /// The two layers of `Option` mean different things: the outer one
    /// reports whether the stack held anything at all, and the inner one
    /// whether the entry it held was a title or the absence of one.
    ///
    /// # Control Functions
    ///
    /// - `XTWINOPS` (`CSI 23 t`); the icon/window selector and the
    ///   direct slot number xterm accepts after it are both ignored, so
    ///   a slot fetch arrives here as an ordinary pop.
    pub fn pop_title(&mut self) -> Option<Option<String>> {
        self.title.stack.pop_back()
    }

    /// Returns the grid dimensions of the active screen.
    pub fn grid_size(&self) -> GridSize {
        self.active_screen().grid_size()
    }

    /// Scrollback rows the active viewport sits above the live tail.
    pub fn display_offset(&self) -> DisplayOffset {
        self.active_screen().display_offset()
    }

    /// The cursor an emitted frame carries: the active screen's write
    /// position with this device's DECTCEM state folded in.
    ///
    /// A frame-ready cursor must be read through here rather than by
    /// pairing a screen read with a separately-read mode.
    pub fn cursor(&self) -> Cursor {
        self.active_screen().cursor(self.modes.text_cursor_enable)
    }

    /// Snapshot of the modes the device owns.
    pub fn modes(&self) -> VtModes {
        self.modes
    }

    /// Mutably borrows the modes the device owns.
    pub fn modes_mut(&mut self) -> &mut VtModes {
        &mut self.modes
    }

    /// Applies `DECAWM`, disarming each screen's deferred wrap when the
    /// mode is reset.
    ///
    /// The mode is device-wide, so a reset disarms both screens rather
    /// than only the one shown. A set leaves an armed flag alone: DEC
    /// STD-070 lists only the reset direction among the operations that
    /// clear the last-column flag.
    ///
    /// The disarm does not reach a checkpoint, so a later restore
    /// (`DECRC`, 1048, 1049) puts the saved flag back.
    ///
    /// # Control Functions
    ///
    /// - `DECAWM` (`CSI ? 7 h` / `CSI ? 7 l`)
    pub fn set_auto_wrap(&mut self, auto_wrap: AutoWrap) {
        self.modes.auto_wrap = auto_wrap;
        if !auto_wrap.wraps() {
            self.screens.primary.disarm_pending_wrap();
            self.screens.alternate.disarm_pending_wrap();
        }
    }

    /// The live palette symbolic colors resolve against.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Sets palette slot `index` to `color`; returns whether the slot
    /// changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 4 ; c ; spec`
    pub fn set_indexed_color(&mut self, index: u8, color: Rgb) -> bool {
        self.palette.set_indexed(index, color)
    }

    /// Returns palette slot `index` to its xterm default; returns
    /// whether the slot changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 104 ; c`
    pub fn reset_indexed_color(&mut self, index: u8) -> bool {
        self.palette.reset_indexed(index)
    }

    /// Returns every palette slot to its xterm default; returns whether
    /// any slot changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 104` with no colour number
    pub fn reset_indexed_colors(&mut self) -> bool {
        self.palette.reset_all_indexed()
    }

    /// Switches the active screen without a flip's side effects.
    #[cfg(test)]
    pub(crate) fn set_active_screen_for_test(&mut self, kind: ScreenKind) {
        self.modes.active_screen = kind;
    }
}

/// Webview placements.
///
/// The cap counts both screens, and a live id is unique across the
/// pair.
impl DeviceState {
    /// Registers a mount at the active screen's cursor under the id the
    /// host minted; `false` when the cap rejects it.
    ///
    /// # Invariants
    ///
    /// A re-mount of a live id frees the slot it takes, so it succeeds
    /// even at the cap.
    pub fn mount_placement(&mut self, size: PlacementSize, id: InstanceId) -> bool {
        self.supersede_placement(id);
        if MAX_PLACEMENTS <= self.placement_count() {
            return false;
        }
        self.active_screen_mut().mount_placement(id, size);
        true
    }

    /// Registers a mount anchored at the active screen's visible cell
    /// (`row`, `column`) under the id the host minted; `false` when the
    /// cell lies outside the grid or the cap rejects it.
    ///
    /// # Invariants
    ///
    /// A re-mount of a live id frees the slot it takes, so it succeeds
    /// even at the cap.
    pub fn mount_placement_at(
        &mut self,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
        id: InstanceId,
    ) -> bool {
        // NOTE: the bounds check must precede supersession. `Grid::line_id`
        // indexes the ring unchecked and panics on a row past the grid, and
        // supersession drops the live placement under `id`, so a rejected
        // re-mount must return here and leave that placement untouched.
        let grid = self.active_screen().grid_size();
        if grid.rows <= row.0 || grid.cols <= column.0 {
            return false;
        }
        self.supersede_placement(id);
        if MAX_PLACEMENTS <= self.placement_count() {
            return false;
        }
        self.active_screen_mut()
            .mount_placement_at(id, row, column, size);
        true
    }

    /// Removes the placement a client `unmount` addresses on either
    /// screen; returns whether anything went.
    ///
    /// An unmount-all removes the matching placements on both screens.
    pub fn unmount_placement(&mut self, id: Option<InstanceId>) -> bool {
        let primary = self.screens.primary.unmount_placement(id);
        let alternate = self.screens.alternate.unmount_placement(id);
        primary || alternate
    }

    /// Removes the placements the host names on either screen; returns
    /// whether anything went.
    pub fn remove_placements(&mut self, ids: &[InstanceId]) -> bool {
        let primary = self.screens.primary.remove_placements(ids);
        let alternate = self.screens.alternate.remove_placements(ids);
        primary || alternate
    }

    /// Sweeps both screens for placements whose anchor no longer resolves
    /// and names them.
    ///
    /// # Invariants
    ///
    /// The primary screen's ids come first.
    pub fn evict_lost_anchors(&mut self) -> Vec<InstanceId> {
        let mut evicted = self.screens.primary.evict_lost_anchors();
        evicted.extend(self.screens.alternate.evict_lost_anchors());
        evicted
    }

    /// Applies an alternate-screen flip, tearing down the placements and
    /// the selection the abandoned alternate screen owned.
    ///
    /// Primary placements and the primary selection are hidden while the
    /// alternate screen is shown, not destroyed. This operation stages no
    /// damage of its own: the caller must stage `DamageSpan::Full` for
    /// the flip.
    pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<InstanceId> {
        self.modes.active_screen = to;
        match to {
            ScreenKind::Alternate => Vec::new(),
            ScreenKind::Primary => {
                self.screens.alternate.clear_selection();
                self.screens.alternate.take_placements()
            }
        }
    }

    fn supersede_placement(&mut self, id: InstanceId) {
        self.screens.primary.supersede_placement(id);
        self.screens.alternate.supersede_placement(id);
    }

    /// Live placements across both screens — what the cap counts.
    fn placement_count(&self) -> usize {
        self.screens.primary.placement_count() + self.screens.alternate.placement_count()
    }
}

/// The primary / alternate pair.
struct Screens {
    primary: Screen,
    alternate: Screen,
}

/// The current window title and the stack `CSI 22 t` saves it on.
#[derive(Default)]
struct TitleState {
    current: Option<String>,
    stack: VecDeque<Option<String>>,
}

/// Titles `CSI 22 t` may stack before the oldest is dropped.
///
/// xterm documents direct stack access over slots 1 through 10, which
/// this bound covers.
const MAX_TITLE_DEPTH: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::modes::InsertReplaceMode;
    use crate::screen::cell::Cell;
    use crate::screen::grid::coords::GridColumn;
    use crate::screen::viewport::ViewportLine;
    use std::iter::from_fn;

    fn device() -> DeviceState {
        DeviceState::new(GridSize { cols: 8, rows: 3 }, 10)
    }

    fn mount(device: &mut DeviceState, id: InstanceId) -> bool {
        device.mount_placement(PlacementSize { rows: 2, cols: 4 }, id)
    }

    /// Asserts that a scroll moves the screen on show and leaves the
    /// other one where it was.
    ///
    /// Case: the user scrolls back through shell output, then a
    /// full-screen editor takes over the alternate screen.
    #[test]
    fn a_scroll_moves_only_the_screen_on_show() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        for _ in 0..5 {
            device.active_screen_mut().move_cursor_to(Some(3), None);
            device.active_screen_mut().line_feed();
        }
        assert_eq!(device.scroll(Scroll::Top), Some(DamageSpan::Full));
        assert_eq!(device.display_offset(), DisplayOffset(5));
        device.switch_screen(ScreenKind::Alternate);
        assert_eq!(device.display_offset(), DisplayOffset(0));
    }

    /// Asserts that a scroll on the alternate screen reports nothing.
    ///
    /// Case: the user rolls the wheel while a full-screen editor is
    /// showing and alternate-scroll translation is off.
    #[test]
    fn a_scroll_on_the_alternate_screen_reports_nothing() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        device.switch_screen(ScreenKind::Alternate);
        assert_eq!(device.scroll(Scroll::Top), None);
        assert_eq!(device.scroll(Scroll::PageUp), None);
        assert_eq!(device.display_offset(), DisplayOffset(0));
    }

    /// Asserts that a resize to the size the device already has
    /// reports no damage.
    ///
    /// Case: the window manager replays the same geometry after a
    /// focus change, so the host forwards a size the VT already holds.
    #[test]
    fn a_resize_to_the_current_size_reports_nothing() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        assert_eq!(device.resize(GridSize { cols: 4, rows: 3 }), None);
    }

    /// Asserts that a resize reports full damage and applies to both
    /// screens, not only the one on show.
    ///
    /// Case: the user resizes the window while a full-screen editor is
    /// running, then quits it back to the shell.
    #[test]
    fn a_resize_applies_to_both_screens() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        assert_eq!(
            device.resize(GridSize { cols: 8, rows: 5 }),
            Some(DamageSpan::Full)
        );
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 8, rows: 5 }
        );
        device.switch_screen(ScreenKind::Alternate);
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 8, rows: 5 }
        );
    }

    /// Asserts that a shrink deep enough to drop an anchor row out of
    /// history leaves that placement evictable.
    ///
    /// Case: a webview is mounted near the top of a short-scrollback
    /// window and the user drags the window much shorter.
    #[test]
    fn a_shrink_past_the_history_cap_leaves_its_placements_evictable() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 4 }, 0);
        let id = InstanceId(1);
        assert!(mount(&mut device, id));
        device.active_screen_mut().move_cursor_to(Some(4), None);
        assert_eq!(
            device.resize(GridSize { cols: 4, rows: 2 }),
            Some(DamageSpan::Full)
        );
        assert_eq!(device.evict_lost_anchors(), vec![id]);
    }

    /// Asserts that a stop set on one screen is absent from the other,
    /// each screen keeping its own tab stop table rather than sharing one.
    ///
    /// Case: a shell installs its own tab positions, then a full-screen
    /// editor takes over the alternate screen and emits a tab.
    #[test]
    fn the_two_screens_carry_independent_tab_stops() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device
                .active_screen_mut()
                .print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
        }
        device.active_screen_mut().set_horizontal_tab_stop();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.active_screen_mut().move_forward_tabs(1);
        assert_eq!(device.active_screen().cursor_column(), GridColumn(8));

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.active_screen_mut().carriage_return();
        device.active_screen_mut().move_forward_tabs(1);
        assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
    }

    /// Asserts that a checkpoint saved on one screen is unreachable from
    /// the other, so a restore on the alternate screen returns its own
    /// power-up state instead of consuming the save the shell left on the
    /// primary.
    ///
    /// Case: a shell saves its cursor, a full-screen editor takes over
    /// the alternate screen and emits a restore of its own, and the
    /// shell then restores after the editor exits.
    #[test]
    fn the_two_screens_carry_independent_checkpoints() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device
                .active_screen_mut()
                .print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
        }
        device.active_screen_mut().save_checkpoint();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device
            .active_screen_mut()
            .print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
        device.active_screen_mut().restore_checkpoint();
        assert_eq!(device.active_screen().cursor_column(), GridColumn(0));

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.active_screen_mut().carriage_return();
        device.active_screen_mut().restore_checkpoint();
        assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
    }

    /// Asserts that a reset clears both screens rather than only the one
    /// the device is showing.
    ///
    /// Case: a full-screen editor leaves its buffer on the alternate
    /// screen, the user quits back to the shell, and the shell then
    /// sends `ESC c`.
    #[test]
    fn a_reset_clears_both_screens() {
        let mut device = device();
        device
            .active_screen_mut()
            .print('p', InsertReplaceMode::Replace, AutoWrap::Enabled);
        device.set_active_screen_for_test(ScreenKind::Alternate);
        device
            .active_screen_mut()
            .print('a', InsertReplaceMode::Replace, AutoWrap::Enabled);

        let _ = device.reset();

        for kind in [ScreenKind::Primary, ScreenKind::Alternate] {
            device.set_active_screen_for_test(kind);
            let row = device.active_screen().viewport_row(ViewportLine(0));
            assert!(row.iter().all(|cell| *cell == Cell::default()));
        }
    }

    /// Asserts that a reset returns the device to the primary screen.
    ///
    /// Case: a full-screen application is killed while it still holds
    /// the alternate screen, and the user runs `reset` to get a usable
    /// shell back.
    #[test]
    fn a_reset_returns_the_device_to_the_primary_screen() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);

        let _ = device.reset();

        assert_eq!(device.modes().active_screen, ScreenKind::Primary);
    }

    /// Asserts that a reset of an untouched device reports no repaint.
    ///
    /// Case: a login script runs `tput reset` before anything has been
    /// printed to the terminal.
    #[test]
    fn a_reset_of_an_untouched_device_reports_no_repaint() {
        assert_eq!(device().reset(), None);
    }

    /// Asserts that a reset reports a full repaint for the content the
    /// primary screen carried.
    ///
    /// Case: a program leaves garbage across the shell's screen and the
    /// user runs `reset` to clear it.
    #[test]
    fn a_reset_of_a_written_primary_screen_reports_a_full_repaint() {
        let mut device = device();
        device
            .active_screen_mut()
            .print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);

        assert_eq!(device.reset(), Some(DamageSpan::Full));
    }

    /// Asserts that a reset reports a full repaint whenever the
    /// alternate screen was showing, even with nothing on the primary
    /// screen behind it.
    ///
    /// Case: a full-screen application that never wrote to the primary
    /// screen is reset while it still holds the alternate screen.
    #[test]
    fn a_reset_of_a_shown_alternate_screen_reports_a_full_repaint() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);

        assert_eq!(device.reset(), Some(DamageSpan::Full));
    }

    /// Asserts that a reset does not report the hidden screen's damage.
    ///
    /// Case: a full-screen editor leaves its buffer on the alternate
    /// screen, the user quits back to a shell screen that nothing has
    /// been printed to, and a startup script then sends `ESC c`.
    #[test]
    fn a_reset_does_not_report_the_hidden_screens_damage() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);
        device
            .active_screen_mut()
            .print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
        device.set_active_screen_for_test(ScreenKind::Primary);

        assert_eq!(device.reset(), None);
    }

    /// Asserts that the cap counts both screens, so a mount is rejected
    /// once the pair holds `MAX_PLACEMENTS` between them.
    ///
    /// Case: a program fills the primary screen with webviews, flips to
    /// the alternate screen, and keeps mounting there.
    #[test]
    fn a_mount_at_the_cap_is_rejected_across_both_screens() {
        let mut device = device();
        let per_screen = MAX_PLACEMENTS / 2;
        for index in 0..per_screen {
            assert!(mount(&mut device, InstanceId(index as u128)));
        }
        device.set_active_screen_for_test(ScreenKind::Alternate);
        for index in 0..MAX_PLACEMENTS - per_screen {
            assert!(mount(&mut device, InstanceId(1000 + index as u128)));
        }
        assert!(!mount(&mut device, InstanceId(9999)));
    }

    /// Asserts that a re-mount of the same id supersedes across the
    /// screen pair rather than leaving a twin on the other screen.
    ///
    /// Case: a program mounts a view on the primary screen, flips
    /// to the alternate screen, and re-mounts the same id there.
    #[test]
    fn a_remount_supersedes_across_screens() {
        let mut device = device();
        let id = InstanceId(1);
        assert!(mount(&mut device, id));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(mount(&mut device, id));
        assert_eq!(device.placement_count(), 1);
    }

    /// Asserts that a broad unmount reaches both screens rather than
    /// stopping at the first match.
    ///
    /// Case: a program mounted a view on each screen and exits, so the
    /// host despawns every child across the terminal in one pass.
    #[test]
    fn a_broad_unmount_reaches_both_screens() {
        let mut device = device();
        assert!(mount(&mut device, InstanceId(1)));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(mount(&mut device, InstanceId(2)));
        assert!(device.unmount_placement(None));
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that a host removal naming one instance per screen clears
    /// both rather than stopping at the first match, and reports that
    /// something went.
    ///
    /// Case: a program that mounted a view on each screen disconnects from
    /// the control socket, so the host names every instance it registered
    /// in one removal.
    #[test]
    fn a_host_removal_reaches_both_screens() {
        let mut device = device();
        let primary = InstanceId(1);
        let alternate = InstanceId(2);
        assert!(mount(&mut device, primary));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(mount(&mut device, alternate));

        assert!(device.remove_placements(&[primary, alternate]));
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that the sweep reaches the inactive screen, so a placement
    /// whose anchor died there is still reclaimed.
    ///
    /// Case: `RIS` resets both screens while a webview is mounted on the
    /// one that is not currently shown.
    #[test]
    fn a_sweep_reaches_the_inactive_screen() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let id = InstanceId(1);
        assert!(mount(&mut device, id));
        assert_eq!(device.active_screen_mut().reset(), None);
        device.set_active_screen_for_test(ScreenKind::Primary);
        assert_eq!(device.evict_lost_anchors(), vec![id]);
    }

    /// Asserts that a reset leaves every placement unresolvable, so one
    /// terminal-wide sweep names all of them.
    ///
    /// Case: an application sends `RIS` while webviews are mounted on
    /// both the primary and the alternate screen.
    #[test]
    fn a_reset_leaves_every_placement_unresolvable() {
        let mut device = device();
        let primary = InstanceId(1);
        assert!(mount(&mut device, primary));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let alternate = InstanceId(2);
        assert!(mount(&mut device, alternate));

        let _ = device.reset();

        assert_eq!(device.evict_lost_anchors(), vec![primary, alternate]);
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that a re-mount of a live id is accepted at the cap.
    ///
    /// Case: a program holding the terminal's last placement slot
    /// re-renders that same view.
    #[test]
    fn re_mounting_a_live_address_succeeds_at_the_cap() {
        let mut device = device();
        for index in 0..MAX_PLACEMENTS {
            assert!(mount(&mut device, InstanceId(index as u128)));
        }
        assert!(mount(&mut device, InstanceId(0)));
        assert_eq!(device.placement_count(), MAX_PLACEMENTS);
    }

    /// Asserts that a flip back to the primary screen tears down the
    /// placements the abandoned alternate screen owned and leaves the
    /// primary's alone.
    ///
    /// Case: a full-screen application that mounted a webview exits while
    /// the shell's own webview from before it is still mounted.
    #[test]
    fn a_flip_to_primary_tears_down_only_the_alternate_placements() {
        let mut device = device();
        let kept = InstanceId(1);
        assert!(mount(&mut device, kept));
        assert!(device.switch_screen(ScreenKind::Alternate).is_empty());
        let dropped = InstanceId(2);
        assert!(mount(&mut device, dropped));
        assert_eq!(device.switch_screen(ScreenKind::Primary), vec![dropped]);
        assert_eq!(device.placement_count(), 1);
        assert_eq!(device.active_screen_mut().take_placements(), vec![kept]);
    }

    /// Asserts that a set title reads back.
    ///
    /// Case: a shell prompt sets the window title and the host asks the
    /// device what it now says.
    #[test]
    fn a_set_title_reads_back() {
        let mut device = device();
        device.set_title(Some("hi".to_owned()));
        assert_eq!(device.title(), Some("hi"));
    }

    /// Asserts that a pushed title comes back off the stack.
    ///
    /// Case: a full-screen editor saves the title, sets its own, and
    /// restores the shell's on the way out.
    #[test]
    fn a_pushed_title_comes_back() {
        let mut device = device();
        device.set_title(Some("shell".to_owned()));
        device.push_title();
        device.set_title(Some("editor".to_owned()));
        assert_eq!(device.pop_title(), Some(Some("shell".to_owned())));
    }

    /// Asserts that popping an empty stack reports that it was empty.
    ///
    /// Case: a program restores a title it never saved.
    #[test]
    fn popping_an_empty_stack_reports_it() {
        let mut device = device();
        assert_eq!(device.pop_title(), None);
    }

    /// Asserts that a title pushed before any title was set comes back
    /// as the absence of one.
    ///
    /// Case: a program saves the title at startup, before the shell has
    /// set any, then restores it.
    #[test]
    fn pushing_before_any_title_pops_an_absence() {
        let mut device = device();
        device.push_title();
        device.set_title(Some("editor".to_owned()));
        assert_eq!(device.pop_title(), Some(None));
    }

    /// Asserts that a full stack drops its oldest entry rather than
    /// refusing the newest, and holds exactly its cap.
    ///
    /// Case: a runaway program pushes titles in a loop.
    #[test]
    fn a_full_stack_drops_its_oldest_entry() {
        let mut device = device();
        for n in 0..=MAX_TITLE_DEPTH {
            device.set_title(Some(n.to_string()));
            device.push_title();
        }
        let popped: Vec<Option<String>> = from_fn(|| device.pop_title()).collect();
        let expected: Vec<Option<String>> = (1..=MAX_TITLE_DEPTH)
            .rev()
            .map(|n| Some(n.to_string()))
            .collect();
        assert_eq!(popped, expected);
    }

    /// Asserts that a reset clears the title and its stack.
    ///
    /// Case: the shell sends `RIS` after a program left both a title
    /// and a saved one behind.
    #[test]
    fn a_reset_clears_the_title_and_its_stack() {
        let mut device = device();
        device.set_title(Some("shell".to_owned()));
        device.push_title();
        let _ = device.reset();
        assert_eq!(device.title(), None);
        assert_eq!(device.pop_title(), None);
    }

    /// Fills the active screen's first row to its last column, arming
    /// the deferred wrap.
    fn arm_deferred_wrap(device: &mut DeviceState) {
        for c in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] {
            device
                .active_screen_mut()
                .print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
        }
    }

    /// The glyph at `column` of the active screen's `line`th visible row.
    fn glyph_at(device: &DeviceState, line: u16, column: u16) -> char {
        device.active_screen().viewport_row(ViewportLine(line))[column].c
    }

    /// Asserts that resetting autowrap disarms the deferred wrap on the
    /// hidden screen as well as the shown one, so a later set cannot
    /// cash in a latch armed before the reset.
    ///
    /// Case: a full-screen application fills the last column of the
    /// primary screen, enters the alternate screen, and turns autowrap
    /// off and on again there.
    #[test]
    fn resetting_autowrap_disarms_the_deferred_wrap_on_both_screens() {
        let mut device = device();
        arm_deferred_wrap(&mut device);
        device.set_active_screen_for_test(ScreenKind::Alternate);
        arm_deferred_wrap(&mut device);

        device.set_auto_wrap(AutoWrap::Disabled);
        device.set_auto_wrap(AutoWrap::Enabled);

        device
            .active_screen_mut()
            .print('z', InsertReplaceMode::Replace, AutoWrap::Enabled);
        assert_eq!(glyph_at(&device, 0, 7), 'z');
        assert_eq!(glyph_at(&device, 1, 0), ' ');

        device.set_active_screen_for_test(ScreenKind::Primary);
        device
            .active_screen_mut()
            .print('z', InsertReplaceMode::Replace, AutoWrap::Enabled);
        assert_eq!(glyph_at(&device, 0, 7), 'z');
        assert_eq!(glyph_at(&device, 1, 0), ' ');
    }

    /// Asserts that resetting autowrap leaves the saved cursor's
    /// deferred wrap alone rather than clearing it, so `DECRC` puts back
    /// the state `DECSC` captured.
    ///
    /// Case: an application fills a row, saves the cursor, turns
    /// autowrap off and on again, and restores the cursor.
    #[test]
    fn resetting_autowrap_leaves_the_saved_deferred_wrap_alone() {
        let mut device = device();
        arm_deferred_wrap(&mut device);
        device.active_screen_mut().save_checkpoint();

        device.set_auto_wrap(AutoWrap::Disabled);
        device.set_auto_wrap(AutoWrap::Enabled);
        device.active_screen_mut().restore_checkpoint();

        device
            .active_screen_mut()
            .print('z', InsertReplaceMode::Replace, AutoWrap::Enabled);
        assert_eq!(glyph_at(&device, 1, 0), 'z');
    }

    /// Asserts that setting autowrap leaves an armed deferred wrap alone
    /// rather than disarming it, so the next character still wraps.
    ///
    /// Case: an application fills a row and re-sends `DECSET 7` while
    /// autowrap is already on.
    #[test]
    fn setting_autowrap_leaves_an_armed_deferred_wrap_alone() {
        let mut device = device();
        arm_deferred_wrap(&mut device);

        device.set_auto_wrap(AutoWrap::Enabled);

        device
            .active_screen_mut()
            .print('z', InsertReplaceMode::Replace, AutoWrap::Enabled);
        assert_eq!(glyph_at(&device, 1, 0), 'z');
    }

    /// Asserts that a reset returns a recolored slot to its xterm
    /// default.
    ///
    /// Case: the user runs `reset` after a theme script recolored ANSI
    /// red.
    #[test]
    fn a_reset_restores_the_palette() {
        let mut device = device();
        device.set_indexed_color(1, Rgb { r: 1, g: 2, b: 3 });
        let _ = device.reset();
        assert_eq!(device.palette().indexed[1], Palette::default().indexed[1]);
    }

    /// Asserts that a reset reports a full repaint when it restores a
    /// recolored slot, even with nothing printed on either screen.
    ///
    /// Case: a theme script recolors the palette in a fresh terminal,
    /// and the user runs `reset` before anything is printed.
    #[test]
    fn a_reset_that_restores_the_palette_reports_a_full_repaint() {
        let mut device = device();
        device.set_indexed_color(1, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(device.reset(), Some(DamageSpan::Full));
    }
}
