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

pub(crate) mod color;
pub(crate) mod modes;

use crate::device::color::Palette;
use crate::device::modes::{ScreenKind, VtModes};
use crate::frame::damage::DamageSpan;
use crate::placement::{MAX_PLACEMENTS, PlacementId, PlacementSize};
use crate::screen::Screen;
use crate::screen::grid::GridSize;
use crate::screen::viewport::{DisplayOffset, Scroll};

/// The emulated terminal device: screens, modes, tabs, colors, title,
/// and the terminal-scoped placement invariants.
///
/// It owns no parser, damage, or emission state — those are the VT's own
/// machinery and sit beside it in [`crate::OrzmaVt`]. The placement
/// table itself belongs to each [`Screen`]; what lives here is only what
/// one screen cannot decide alone: the id counter, the cap across the
/// pair, and the `(view_id, instance_id)` address space.
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
    next_placement_id: PlacementId,
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
            colors: ColorTable {
                palette: Palette::default(),
            },
            title: TitleState {},
            next_placement_id: PlacementId(0),
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

    /// Resizes both screens, reflowing content; `None` when the
    /// dimensions already matched.
    ///
    /// # Invariants
    ///
    /// A resize that changes the dimensions must report
    /// [`DamageSpan::Full`]: every emitted frame carries the new size but
    /// nothing diffs it, so partial row damage would hand the renderer
    /// new dimensions with stale rows behind them.
    // TODO: A rewrap can insert or drop rows in the middle of the ring,
    // and can split or merge them, so a surviving placement anchor has to
    // be told which resulting row it now belongs to. Once reflow lands,
    // route the row remapping it produces to each screen's placement
    // table so it can re-anchor each placement to its surviving row.
    pub fn resize(&mut self, _size: GridSize) -> Option<DamageSpan> {
        todo!()
    }

    /// Moves the active viewport; `None` for a clamped or zero motion.
    ///
    /// # Invariants
    ///
    /// A motion that moves the viewport must report [`DamageSpan::Full`]:
    /// the emit-time offset diff only guarantees that a frame is
    /// emitted, not that it carries rows, so anything less would
    /// repaint stale content at the new offset.
    pub fn scroll(&mut self, _scroll: Scroll) -> Option<DamageSpan> {
        todo!()
    }

    /// Returns both screens and every mode to their power-up state;
    /// `None` when the frame that follows needs no repaint.
    ///
    /// # Invariants
    ///
    /// Only the screen left active by the mode reset reaches a frame, so
    /// the alternate screen's damage is dropped rather than folded in.
    /// A reset that arrives while the alternate screen is shown always
    /// repaints, because the implicit return to the primary screen
    /// replaces the whole viewport.
    ///
    /// # Control Functions
    ///
    /// - `RIS` (`ESC c`)
    pub fn reset(&mut self) -> Option<DamageSpan> {
        let was_showing_alternate = matches!(self.modes.active_screen, ScreenKind::Alternate);
        let primary = self.screens.primary.reset();
        let _ = self.screens.alternate.reset();
        self.modes = VtModes::default();
        (was_showing_alternate || primary.is_some()).then_some(DamageSpan::Full)
    }

    /// Returns the grid dimensions of the active screen.
    pub fn grid_size(&self) -> GridSize {
        self.active_screen().grid_size()
    }

    /// Scrollback rows the active viewport sits above the live tail.
    pub fn display_offset(&self) -> DisplayOffset {
        self.active_screen().display_offset()
    }

    /// Snapshot of the input-relevant device modes.
    pub fn modes(&self) -> VtModes {
        self.modes
    }

    /// The live palette symbolic colors resolve against.
    pub fn palette(&self) -> &Palette {
        &self.colors.palette
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

/// Webview placements.
///
/// The three terminal-scoped invariants live here because none of them
/// can be satisfied by one screen alone: ids are minted per terminal,
/// the cap counts both screens, and a `(view_id, instance_id)` address
/// is unique across the pair.
impl DeviceState {
    /// Registers a mount at the active screen's cursor and mints its id;
    /// `None` when the cap rejects it.
    ///
    /// # Invariants
    ///
    /// Supersession runs before the cap check: a re-mount frees the slot
    /// it takes, so it must succeed even at the limit.
    pub fn mount_placement(
        &mut self,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) -> Option<PlacementId> {
        self.supersede_placement(&view_id, instance_id.as_deref());
        if MAX_PLACEMENTS <= self.placement_count() {
            return None;
        }
        let id = self.mint_placement_id();
        self.active_screen_mut()
            .mount_placement(id, size, view_id, instance_id);
        Some(id)
    }

    /// Removes the placements a client `unmount` addresses on either
    /// screen; returns whether anything went.
    ///
    /// # Invariants
    ///
    /// Every screen is visited — the accumulation must not short-circuit.
    /// A broad scope (`view_id` alone, or unmount-all) can match on both
    /// screens, and the host despawns every matching child across the
    /// terminal in one pass, so a VT that stopped at the first match
    /// would keep a placement holding a cap slot whose host child is
    /// already gone.
    pub fn unmount_placement(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool {
        let primary = self.screens.primary.unmount_placement(view_id, instance_id);
        let alternate = self
            .screens
            .alternate
            .unmount_placement(view_id, instance_id);
        primary || alternate
    }

    /// Sweeps both screens for placements whose anchor no longer resolves
    /// and names them.
    ///
    /// # Invariants
    ///
    /// The primary screen's ids come first. The order is observable —
    /// the host acts on the returned list in sequence — and this is the
    /// only operation that exposes it, so it is fixed here.
    pub fn evict_lost_anchors(&mut self) -> Vec<PlacementId> {
        let mut evicted = self.screens.primary.evict_lost_anchors();
        evicted.extend(self.screens.alternate.evict_lost_anchors());
        evicted
    }

    /// Applies an alternate-screen flip, tearing down the placements the
    /// abandoned alternate screen owned.
    ///
    /// Primary placements are hidden while the alternate screen is shown,
    /// not destroyed. This operation stages no damage of its own: the
    /// flip itself must stage `DamageSpan::Full`, which carries the
    /// changed list.
    pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<PlacementId> {
        self.modes.active_screen = to;
        match to {
            ScreenKind::Alternate => Vec::new(),
            ScreenKind::Primary => self.screens.alternate.take_placements(),
        }
    }

    /// Drops the placement a re-mount replaces on either screen.
    fn supersede_placement(&mut self, view_id: &str, instance_id: Option<&str>) {
        self.screens
            .primary
            .supersede_placement(view_id, instance_id);
        self.screens
            .alternate
            .supersede_placement(view_id, instance_id);
    }

    /// Live placements across both screens — what the cap counts.
    fn placement_count(&self) -> usize {
        self.screens.primary.placement_count() + self.screens.alternate.placement_count()
    }

    /// Hands out the next unused placement id.
    ///
    /// # Invariants
    ///
    /// Ids only ever move forward, which is what lets a delayed
    /// id-addressed lifecycle event be matched against a placement that
    /// may already be gone; the overflow guard is what keeps that true.
    fn mint_placement_id(&mut self) -> PlacementId {
        let id = self.next_placement_id;
        self.next_placement_id = PlacementId(
            id.0.checked_add(1)
                .expect("a session cannot mint u64::MAX placements"),
        );
        id
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
// TODO: Apply the OSC 4 / 10 / 11 / 12 overrides to the carried
// palette once their handlers land.
struct ColorTable {
    palette: Palette,
}

/// The current window title and its stack.
// TODO: Carry the current title plus the CSI 22 / 23 t stack.
struct TitleState {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::cell::Cell;
    use crate::screen::grid::coords::GridColumn;
    use crate::screen::viewport::ViewportLine;

    fn device() -> DeviceState {
        DeviceState::new(GridSize { cols: 8, rows: 3 }, 10)
    }

    fn mount(device: &mut DeviceState, view: &str) -> Option<PlacementId> {
        device.mount_placement(PlacementSize { rows: 2, cols: 4 }, view.to_string(), None)
    }

    /// Asserts that a stop set on one screen is absent from the other.
    ///
    /// The agreed policy gives each screen its own table, so a
    /// full-screen application cannot disturb the tab positions the
    /// shell left on the primary screen. xterm, VTE, alacritty,
    /// wezterm, and Windows Terminal share one table across both
    /// screens instead; ECMA-48 settles nothing here, because it
    /// has no alternate screen at all.
    ///
    /// Case: a shell installs its own tab positions, then a full-screen
    /// editor takes over the alternate screen and emits a tab.
    #[test]
    fn the_two_screens_carry_independent_tab_stops() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device.active_screen_mut().print(c);
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
            device.active_screen_mut().print(c);
        }
        device.active_screen_mut().save_checkpoint();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.active_screen_mut().print('x');
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
        device.active_screen_mut().print('p');
        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.active_screen_mut().print('a');

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
        device.active_screen_mut().print('x');

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
        device.active_screen_mut().print('x');
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
            assert!(mount(&mut device, &format!("p{index}")).is_some());
        }
        device.set_active_screen_for_test(ScreenKind::Alternate);
        for index in 0..MAX_PLACEMENTS - per_screen {
            assert!(mount(&mut device, &format!("a{index}")).is_some());
        }
        assert!(mount(&mut device, "one-too-many").is_none());
    }

    /// Asserts that ids keep moving forward across screens and across
    /// unmounts, so a delayed id-addressed event can never name a
    /// successor.
    ///
    /// Case: a program mounts on one screen, unmounts, flips screens, and
    /// mounts again while the host is still acting on the first id.
    #[test]
    fn placement_ids_are_minted_ascending_across_screens() {
        let mut device = device();
        let first = mount(&mut device, "a").expect("first mount accepted");
        device.unmount_placement(None, None);
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let second = mount(&mut device, "b").expect("second mount accepted");
        assert!(first < second);
    }

    /// Asserts that a re-mount of the same address supersedes across the
    /// screen pair rather than leaving a twin on the other screen.
    ///
    /// Case: a program mounts a named view on the primary screen, flips
    /// to the alternate screen, and re-mounts the same name there.
    #[test]
    fn a_remount_supersedes_across_screens() {
        let mut device = device();
        mount(&mut device, "memo").expect("first mount accepted");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        mount(&mut device, "memo").expect("re-mount accepted");
        assert_eq!(device.placement_count(), 1);
    }

    /// Asserts that a broad unmount reaches both screens rather than
    /// stopping at the first match.
    ///
    /// Case: a program mounted the same view on both screens and exits,
    /// so the host despawns every child at that address in one pass.
    #[test]
    fn a_broad_unmount_reaches_both_screens() {
        let mut device = device();
        mount(&mut device, "memo").expect("primary mount accepted");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        mount(&mut device, "chart").expect("alternate mount accepted");
        assert!(device.unmount_placement(None, None));
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
        let id = mount(&mut device, "memo").expect("alternate mount accepted");
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
        let primary = mount(&mut device, "shell").expect("primary mount accepted");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let alternate = mount(&mut device, "app").expect("alternate mount accepted");

        let _ = device.reset();

        assert_eq!(device.evict_lost_anchors(), vec![primary, alternate]);
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that a reset does not rewind the device's placement id
    /// counter.
    ///
    /// Case: a webview is mounted, the user runs `reset`, and the
    /// program mounts a fresh view while the eviction signal for the old
    /// one is still in flight.
    #[test]
    fn a_reset_does_not_rewind_the_device_placement_id_counter() {
        let mut device = device();
        let old = mount(&mut device, "a").expect("mount accepted");
        assert_eq!(device.reset(), None);
        device.evict_lost_anchors();

        let new = mount(&mut device, "b").expect("mount after reset accepted");

        assert!(old < new);
    }

    /// Asserts that a re-mount of a live address is accepted at the cap,
    /// because supersession frees the slot it takes before the check.
    ///
    /// Case: a program holding the terminal's last placement slot
    /// re-renders that same named view.
    #[test]
    fn re_mounting_a_live_address_succeeds_at_the_cap() {
        let mut device = device();
        for index in 0..MAX_PLACEMENTS {
            mount(&mut device, &format!("v{index}")).expect("a mount under the cap is accepted");
        }
        assert!(mount(&mut device, "v0").is_some());
        assert_eq!(device.placement_count(), MAX_PLACEMENTS);
    }

    /// Asserts that a flip back to the primary screen tears down the
    /// placements the abandoned alternate screen owned and leaves the
    /// primary's alone.
    ///
    /// Case: a full-screen application that mounted a webview exits, and
    /// the shell's own webview from before it must survive.
    #[test]
    fn a_flip_to_primary_tears_down_only_the_alternate_placements() {
        let mut device = device();
        let kept = mount(&mut device, "shell").expect("primary mount accepted");
        assert!(device.switch_screen(ScreenKind::Alternate).is_empty());
        let dropped = mount(&mut device, "app").expect("alternate mount accepted");
        assert_eq!(device.switch_screen(ScreenKind::Primary), vec![dropped]);
        assert_eq!(device.placement_count(), 1);
        assert_eq!(device.active_screen_mut().take_placements(), vec![kept]);
    }
}
