//! Server-side data plane: per-surface terminal drivers and the registry that
//! routes client commands to them.

mod driver;

use crate::terminal::driver::SurfaceDriver;
use crossbeam_channel::{Receiver, Sender, unbounded};
use ozmux_mux::SurfaceId;
use ozmux_proto::{CellSide, CopyModeOp, SelectionKind, ServerMessage, ViMotionKind};
use ozmux_vt::frame::FrameSnapshot;
use ozmux_vt::vt::{Vt, VtMotion, VtSelection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;
use tokio::sync::{broadcast, oneshot};

/// A command sent from an async client task to a surface's driver thread.
pub(crate) enum DriverCommand {
    /// Pre-encoded input bytes to write to the PTY.
    Input(Vec<u8>),
    /// Scroll the host viewport by `delta` rows (positive = into history).
    Scroll(i32),
    /// Resize the PTY and the VT grid.
    Resize { cols: u16, rows: u16 },
    /// A copy-mode op; `reply` is `Some` only for `CopySelection`.
    CopyMode { op: CopyModeOp, reply: Option<oneshot::Sender<String>> },
    /// Cold-attach: reply with a fresh full snapshot.
    Snapshot(oneshot::Sender<FrameSnapshot>),
}

/// A reserved-but-not-yet-spawned driver: its routing slot exists in the
/// registry; the OS thread is started after the structural broadcast.
pub(crate) struct DriverSeed {
    pub(crate) surface: SurfaceId,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) cmd_rx: Receiver<DriverCommand>,
}

struct SurfaceHandle {
    cmd_tx: Sender<DriverCommand>,
    join: Option<JoinHandle<()>>,
}

/// Maps a proto `CopyModeOp` onto the `Vt`'s alacritty-free API. Returns the
/// selection text only for `CopySelection`; otherwise `None`.
pub(crate) fn apply_copy_mode(vt: &mut Vt, op: &CopyModeOp) -> Option<String> {
    match op {
        CopyModeOp::Enter => vt.enter_vi_mode(),
        CopyModeOp::Exit => vt.exit_vi_mode(),
        CopyModeOp::ViMotion(k) => vt.vi_motion_kind(motion_to_vt(*k)),
        CopyModeOp::ViGoto { point } => vt.vi_goto_view(point.line, point.col),
        CopyModeOp::ScrollPageUp => vt.scroll_page_up(),
        CopyModeOp::ScrollPageDown => vt.scroll_page_down(),
        CopyModeOp::SelectionStartAt { point, side, ty } => {
            vt.selection_start_at_view(point.line, point.col, is_right(*side), sel_to_vt(*ty))
        }
        CopyModeOp::SelectionUpdateTo { point, side } => {
            vt.selection_update_to_view(point.line, point.col, is_right(*side))
        }
        CopyModeOp::SelectionStart { ty } => vt.selection_start_kind(sel_to_vt(*ty)),
        CopyModeOp::SelectionClear => vt.selection_clear(),
        CopyModeOp::SelectionChangeType { ty } => {
            vt.selection_change_type_kind(sel_to_vt(*ty));
        }
        CopyModeOp::CopySelection => return vt.selection_to_string(),
    }
    None
}

fn is_right(side: CellSide) -> bool {
    matches!(side, CellSide::Right)
}

fn motion_to_vt(k: ViMotionKind) -> VtMotion {
    match k {
        ViMotionKind::Left => VtMotion::Left,
        ViMotionKind::Right => VtMotion::Right,
        ViMotionKind::Up => VtMotion::Up,
        ViMotionKind::Down => VtMotion::Down,
        ViMotionKind::First => VtMotion::First,
        ViMotionKind::Last => VtMotion::Last,
        ViMotionKind::FirstOccupied => VtMotion::FirstOccupied,
        ViMotionKind::High => VtMotion::High,
        ViMotionKind::Low => VtMotion::Low,
        ViMotionKind::WordRight => VtMotion::WordRight,
        ViMotionKind::WordLeft => VtMotion::WordLeft,
        ViMotionKind::WordRightEnd => VtMotion::WordRightEnd,
    }
}

fn sel_to_vt(k: SelectionKind) -> VtSelection {
    match k {
        SelectionKind::Simple => VtSelection::Simple,
        SelectionKind::Block => VtSelection::Block,
        SelectionKind::Lines => VtSelection::Lines,
        SelectionKind::Semantic => VtSelection::Semantic,
    }
}

/// Routes client commands to per-surface driver threads.
pub(crate) struct TerminalRegistry {
    map: Mutex<HashMap<SurfaceId, SurfaceHandle>>,
}

impl TerminalRegistry {
    /// Creates an empty registry.
    pub(crate) fn new() -> Self {
        Self { map: Mutex::new(HashMap::new()) }
    }

    /// Sends `cmd` to `surface`'s driver. Returns `false` when no driver is
    /// registered (already-closed surface); the caller drops the command.
    pub(crate) fn route(&self, surface: SurfaceId, cmd: DriverCommand) -> bool {
        let map = self.map.lock().unwrap();
        match map.get(&surface) {
            Some(handle) => handle.cmd_tx.send(cmd).is_ok(),
            None => false,
        }
    }

    /// Reserves a routing slot for `surface` and returns the seed used to spawn
    /// the driver thread AFTER the structural broadcast. Empty `cwd` -> `None`.
    /// Returns `None` (skips) when `surface` is already registered (idempotent).
    pub(crate) fn reserve(
        &self,
        surface: SurfaceId,
        cols: u16,
        rows: u16,
        cwd: PathBuf,
    ) -> Option<DriverSeed> {
        let mut map = self.map.lock().unwrap();
        if map.contains_key(&surface) {
            return None;
        }
        let (cmd_tx, cmd_rx) = unbounded();
        map.insert(surface, SurfaceHandle { cmd_tx, join: None });
        let cwd = if cwd.as_os_str().is_empty() { None } else { Some(cwd) };
        Some(DriverSeed { surface, cols, rows, cwd, cmd_rx })
    }

    /// Spawns the driver OS thread for a reserved seed and attaches its join
    /// handle. Must run AFTER the structural broadcast so the driver's first
    /// frame follows the `PaneCreated`/`SurfaceSpawned` event.
    pub(crate) fn spawn(&self, seed: DriverSeed, events_tx: broadcast::Sender<ServerMessage>) {
        let surface = seed.surface;
        let join = SurfaceDriver::spawn(seed, events_tx);
        let mut map = self.map.lock().unwrap();
        if let Some(handle) = map.get_mut(&surface) {
            handle.join = Some(join);
        }
    }

    /// Removes `surface`'s driver: dropping `cmd_tx` disconnects the driver's
    /// command receiver, so its `Select` returns and the thread exits.
    pub(crate) fn remove(&self, surface: SurfaceId) {
        let mut map = self.map.lock().unwrap();
        map.remove(&surface);
    }

    /// True when `surface` has a registered driver.
    pub(crate) fn contains(&self, surface: SurfaceId) -> bool {
        self.map.lock().unwrap().contains_key(&surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn apply_copy_mode_enter_and_exit_return_none() {
        let mut vt = Vt::new(10, 5);
        let out = apply_copy_mode(&mut vt, &CopyModeOp::Enter);
        assert!(out.is_none());
        // Behavioral check: exit should succeed (round-trip) without panicking,
        // confirming Enter armed vi mode.
        let out2 = apply_copy_mode(&mut vt, &CopyModeOp::Exit);
        assert!(out2.is_none());
    }

    #[test]
    fn apply_copy_mode_copyselection_returns_text() {
        let mut vt = Vt::new(10, 3);
        vt.on_output(b"X", Instant::now());
        vt.enter_vi_mode();
        // Move the vi cursor to column 0 (where 'X' was written) before
        // anchoring the selection.
        apply_copy_mode(&mut vt, &CopyModeOp::ViMotion(ViMotionKind::First));
        apply_copy_mode(&mut vt, &CopyModeOp::SelectionStart { ty: SelectionKind::Simple });
        let text = apply_copy_mode(&mut vt, &CopyModeOp::CopySelection);
        assert!(text.unwrap_or_default().contains('X'));
    }

    #[test]
    fn reserve_is_idempotent_for_same_surface() {
        let reg = TerminalRegistry::new();
        let s = SurfaceId::default();
        assert!(reg.reserve(s, 80, 24, PathBuf::new()).is_some());
        assert!(
            reg.reserve(s, 80, 24, PathBuf::new()).is_none(),
            "second reserve must be skipped"
        );
        assert!(reg.contains(s));
    }

    #[test]
    fn route_to_unknown_surface_returns_false() {
        let reg = TerminalRegistry::new();
        assert!(!reg.route(SurfaceId::default(), DriverCommand::Scroll(1)));
    }
}
