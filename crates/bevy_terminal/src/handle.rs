//! `TerminalHandle` — Component wrapping the Bevy-free `ozmux_vt::vt::Vt`
//! engine plus the PTY-coupled transfer methods (`write` / `resize`).
//!
//! The handle owns exactly one `Vt`. Pure VT operations are forwarded to
//! it through thin delegates; the Bevy bridge (`plugin.rs`) drives the
//! engine's data-returning API (`on_output` / `emit` / `tick` /
//! `drain_events` / `drain_replies`) and translates the results into
//! `EntityEvent`s.

use crate::pty::PtyHandle;
use bevy::ecs::component::Component;
use bevy_terminal_renderer::prelude::Cursor;
use ozmux_vt::vt::{ViIndicatorSnapshot, Vt};
use std::time::Instant;

/// VT + PTY bridge state for a single terminal entity.
///
/// Holds the engine (`vt`) and forwards pure VT operations to it. The
/// PTY-coupled methods (`write` / `resize`) live here because they touch
/// the `PtyHandle`, which `Vt` deliberately does not own.
#[derive(Component)]
pub struct TerminalHandle {
    pub(crate) vt: Vt,
}

impl TerminalHandle {
    /// Constructs a fresh handle sized `cols` x `rows`. Called only from
    /// `TerminalBundle::spawn`.
    pub(crate) fn new(cols: u16, rows: u16) -> Self {
        Self {
            vt: Vt::new(cols, rows),
        }
    }

    /// Writes bytes to the PTY master.
    ///
    /// # Invariants
    ///
    /// `note_user_input` is called BEFORE the PTY write so a racing emit
    /// cycle that observes the user input cannot miss the flag. The
    /// coalescer's `AtMostOneRow + pending_user_input` immediate-flush
    /// rule depends on this ordering — without it, keyboard echo degrades
    /// to the IDLE deadline (≈1 Bevy frame).
    pub fn write(&mut self, pty: &mut PtyHandle, bytes: &[u8]) -> std::io::Result<()> {
        self.vt.note_user_input();
        pty.write_all(bytes)
    }

    /// Resizes the alacritty grid and the PTY master together.
    ///
    /// # Invariants
    ///
    /// `Vt::resize` stages Full damage so the new geometry reaches the
    /// renderer even when no PTY output is pending; `check_deadline_flush`
    /// arms that staged damage on the next tick.
    pub fn resize(&mut self, pty: &mut PtyHandle, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.vt.resize(cols, rows);
        pty.resize(cols, rows)?;
        Ok(())
    }

    /// Feeds a chunk of PTY bytes through the VT parser. Test-facing
    /// delegate; production drives the engine via `Vt::on_output`.
    pub fn advance(&mut self, chunk: &[u8]) {
        let _ = self.vt.on_output(chunk, Instant::now());
    }

    /// Returns the current value of the `pending_user_input` flag set by
    /// `write`. Exposed so cross-crate integration tests can confirm that
    /// a PTY write took place without reading from the PTY master.
    pub fn pending_user_input(&self) -> bool {
        self.vt.pending_user_input()
    }

    /// Scrolls the visible viewport by `delta` lines. Positive `delta`
    /// moves backward into scrollback history; negative moves forward
    /// toward the live tail.
    pub fn scroll(&mut self, delta: i32) {
        self.vt.scroll(delta);
    }

    /// Snaps the viewport to the live tail.
    pub fn scroll_to_bottom(&mut self) {
        self.vt.scroll_to_bottom();
    }

    /// Scrolls the viewport one page up.
    pub fn scroll_page_up(&mut self) {
        self.vt.scroll_page_up();
    }

    /// Scrolls the viewport one page down.
    pub fn scroll_page_down(&mut self) {
        self.vt.scroll_page_down();
    }

    /// Enters vi (copy) mode. Idempotent.
    pub fn enter_vi_mode(&mut self) {
        self.vt.enter_vi_mode();
    }

    /// Exits vi mode and snaps the viewport to the live tail. Idempotent.
    pub fn exit_vi_mode(&mut self) {
        self.vt.exit_vi_mode();
    }

    /// Drives `Term::vi_motion(motion)`.
    pub fn vi_motion(&mut self, motion: alacritty_terminal::vi_mode::ViMotion) {
        self.vt.vi_motion(motion);
    }

    /// Jumps the vi cursor to `viewport_point`. No-op when not in vi mode.
    pub fn vi_goto(&mut self, viewport_point: alacritty_terminal::index::Point) {
        self.vt.vi_goto(viewport_point);
    }

    /// Starts a selection of `ty` anchored at `viewport_point` with `side`.
    pub fn selection_start_at(
        &mut self,
        viewport_point: alacritty_terminal::index::Point,
        side: alacritty_terminal::index::Side,
        ty: alacritty_terminal::selection::SelectionType,
    ) {
        self.vt.selection_start_at(viewport_point, side, ty);
    }

    /// Extends the active selection's moving end to `viewport_point` /
    /// `side`. No-op when there is no active selection.
    pub fn selection_update_to(
        &mut self,
        viewport_point: alacritty_terminal::index::Point,
        side: alacritty_terminal::index::Side,
    ) {
        self.vt.selection_update_to(viewport_point, side);
    }

    /// Starts a selection of the given type at the current vi cursor.
    pub fn selection_start(&mut self, ty: alacritty_terminal::selection::SelectionType) {
        self.vt.selection_start(ty);
    }

    /// Drops the active selection.
    pub fn selection_clear(&mut self) {
        self.vt.selection_clear();
    }

    /// Switches the active selection's type while preserving the anchor.
    /// Returns `false` when no selection is active.
    pub fn selection_change_type(
        &mut self,
        ty: alacritty_terminal::selection::SelectionType,
    ) -> bool {
        self.vt.selection_change_type(ty)
    }

    /// Reads the current cols / rows / cursor.
    pub fn read_geometry(&self) -> (u16, u16, Cursor) {
        self.vt.read_geometry()
    }

    /// Returns the current `TermMode` bitflags.
    pub fn current_modes(&self) -> alacritty_terminal::term::TermMode {
        self.vt.current_modes()
    }

    /// True iff `TermMode::APP_CURSOR` (DECCKM) is set.
    pub fn is_app_cursor_keys(&self) -> bool {
        self.vt.is_app_cursor_keys()
    }

    /// True iff `TermMode::BRACKETED_PASTE` is set.
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.vt.bracketed_paste_enabled()
    }

    /// True when the viewport is pinned to the live tail
    /// (`display_offset == 0`).
    pub fn is_at_bottom(&self) -> bool {
        self.vt.is_at_bottom()
    }

    /// Reads the active selection as a UTF-8 string. `None` when no
    /// selection is set or it is empty.
    pub fn selection_to_string(&self) -> Option<String> {
        self.vt.selection_to_string()
    }

    /// Returns the type of the active selection, if any.
    pub fn selection_type(&self) -> Option<alacritty_terminal::selection::SelectionType> {
        self.vt.selection_type()
    }

    /// Returns the current scroll offset and history length for the
    /// copy-mode indicator's `[offset/total]` chip.
    pub fn vi_indicator_snapshot(&self) -> ViIndicatorSnapshot {
        self.vt.vi_indicator_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{SpawnOptions, TerminalBundle};
    use alacritty_terminal::term::TermMode;

    fn spawn_handle(cols: u16, rows: u16) -> TerminalHandle {
        let opts = SpawnOptions {
            cols,
            rows,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        TerminalBundle::spawn(opts).expect("spawn shell").handle
    }

    #[test]
    fn write_sets_pending_user_input() {
        let bundle = TerminalBundle::spawn(SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("spawn /bin/sh");
        let mut handle = bundle.handle;
        let mut pty = bundle.pty;
        assert!(
            !handle.pending_user_input(),
            "fresh handle must have no pending input"
        );
        handle.write(&mut pty, b"x").expect("write");
        assert!(
            handle.pending_user_input(),
            "after write the flag must be true (used by tests to verify a PTY write happened)",
        );
    }

    #[test]
    fn advance_then_bracketed_paste_enabled_reflects_mode() {
        let mut handle = spawn_handle(10, 5);
        assert!(!handle.bracketed_paste_enabled());
        handle.advance(b"\x1b[?2004h");
        assert!(handle.bracketed_paste_enabled());
        handle.advance(b"\x1b[?2004l");
        assert!(!handle.bracketed_paste_enabled());
    }

    #[test]
    fn enter_and_exit_vi_mode_toggle_vi_bit() {
        let mut handle = spawn_handle(10, 5);
        assert!(!handle.current_modes().contains(TermMode::VI));
        handle.enter_vi_mode();
        assert!(handle.current_modes().contains(TermMode::VI));
        handle.exit_vi_mode();
        assert!(!handle.current_modes().contains(TermMode::VI));
    }

    #[test]
    fn selection_start_at_creates_selection() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::SelectionType;
        let mut handle = spawn_handle(80, 24);
        handle.selection_start_at(
            Point::new(Line(5), Column(10)),
            Side::Left,
            SelectionType::Simple,
        );
        assert_eq!(handle.selection_type(), Some(SelectionType::Simple));
        assert!(handle.selection_to_string().is_some());
    }

    #[test]
    fn vi_indicator_snapshot_starts_at_bottom() {
        let handle = spawn_handle(10, 5);
        assert_eq!(handle.vi_indicator_snapshot().scroll_offset, 0);
        assert!(handle.is_at_bottom());
    }
}
