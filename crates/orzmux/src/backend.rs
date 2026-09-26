//! The multiplexer's pane ledger: owns every pane and the layout tree,
//! applies the operations the event loop dispatches, and queues the
//! events the GUI receives.

use crate::backend::layout::LayoutTree;
use crate::backend::pane::{Pane, PaneFactory};
use crate::backend::queue_sample::ChunkDepth;
use crate::error::{OrzmuxError, OrzmuxResult};
use orzma_tty::prelude::{
    OrzmaTty, OrzmaTtyError, OrzmaTtyResult, PointerInput, PumpItem, Readiness, TerminalKey,
    TerminalModifiers, TtySignal, WheelConfig, WheelInput,
};
use orzma_tty::{CellPixels, EnvKey, EnvValue};
use orzma_vt::prelude::{
    Frame, GridColumn, GridSize, InstanceId, OrzmaVt, PlacementSize, ScreenLine, Scroll, Vt,
    VtSignal,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::Level;

pub(crate) mod layout;
pub(crate) mod pane;
pub(crate) mod queue_sample;
pub(crate) use pane::ShellFactory;

/// A pane the backend minted. Never reused within one backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u32);

/// A split the layout tree minted. Never reused within one tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SplitId(pub u32);

/// A GUI-minted correlation id for a `NewPane` request, echoed by the
/// `PaneOpened` / `SpawnFailed` that answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

impl RequestId {
    /// Mints the next id. Process-wide, strictly increasing.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The position of a command in the GUI's send order; `Layout.seq`
/// reports how far the backend has processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CommandSeq(pub u64);

/// Which pane a command addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneTarget {
    /// The backend's active pane at the moment the command is processed.
    Active,
    /// A specific pane.
    Id(PaneId),
}

/// Which way a split divides a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitOrientation {
    /// A vertical divider: the panes end up side by side.
    Vertical,
    /// A horizontal divider: the panes end up stacked.
    Horizontal,
}

/// A direction for selecting a neighbouring pane or moving a divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    /// Toward the left edge of the window.
    Left,
    /// Toward the bottom edge of the window.
    Down,
    /// Toward the top edge of the window.
    Up,
    /// Toward the right edge of the window.
    Right,
}

/// Where a new pane goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewPaneAt {
    /// The first pane; valid only while the tree is empty.
    Root,
    /// Split `pane`, putting the new pane right of / below it.
    Split {
        /// The pane being split.
        pane: PaneTarget,
        /// The direction of the divider the split introduces.
        orientation: SplitOrientation,
    },
}

/// Why a pane closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The shell exited; `code` is `None` when the wait itself failed.
    ChildExit {
        /// The shell's exit code, when the wait could observe it.
        code: Option<i32>,
    },
    /// The GUI killed it.
    Killed,
}

/// One pane's rectangle in whole-window cell coordinates, origin at the
/// window's top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    /// The pane this rectangle belongs to.
    pub pane: PaneId,
    /// Left edge in cells from the window's left.
    pub x: u16,
    /// Top edge in cells from the window's top.
    pub y: u16,
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
}

/// A one-cell-wide divider between two panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separator {
    /// The split this divider belongs to.
    pub split: SplitId,
    /// Whether the divider runs vertically or horizontally.
    pub orientation: SplitOrientation,
    /// Left edge in cells from the window's left.
    pub x: u16,
    /// Top edge in cells from the window's top.
    pub y: u16,
    /// The divider's length in cells.
    pub len: u16,
}

/// The complete pane geometry after one tree mutation or selection.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The last command the backend processed before building this.
    pub seq: CommandSeq,
    /// The extent the panes tile: the window size in cells, widened per
    /// axis to the tree's minimum when the window is smaller.
    pub size: GridSize,
    /// The pane the backend considers active, when any pane exists.
    pub active: Option<PaneId>,
    /// Every pane's rectangle in the current tree.
    pub panes: Vec<PaneRect>,
    /// Every divider between adjacent panes in the current tree.
    pub separators: Vec<Separator>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            seq: CommandSeq::default(),
            size: GridSize { cols: 0, rows: 0 },
            active: None,
            panes: Vec::new(),
            separators: Vec::new(),
        }
    }
}

/// An event the backend emits to the GUI.
#[derive(Debug, Clone, PartialEq)]
pub enum OrzmuxEvent {
    /// A pane requested by `NewPane` was spawned successfully.
    PaneOpened {
        /// The newly spawned pane.
        pane: PaneId,
        /// The `NewPane` request this answers.
        request: RequestId,
    },
    /// A pane requested by `NewPane` failed to spawn.
    SpawnFailed {
        /// The `NewPane` request this answers.
        request: RequestId,
        /// A human-readable description of the failure.
        error: String,
    },
    /// Exactly one per tree mutation or selection. `frames` carries the
    /// immediate repaint of every pane whose size changed.
    Layout {
        /// The new layout snapshot.
        layout: Layout,
        /// The immediate repaint of every pane whose size changed.
        frames: Vec<(PaneId, Frame)>,
    },
    /// A pane produced a new frame outside of a layout change.
    Frame {
        /// The pane that produced the frame.
        pane: PaneId,
        /// The pane's new frame.
        frame: Frame,
    },
    /// A pane's VT emitted a signal the GUI must act on.
    Signal {
        /// The pane the signal came from.
        pane: PaneId,
        /// The signal itself.
        signal: VtSignal,
    },
    /// Exactly one per `CopySelection`; `text` is `None` when the target
    /// could not be resolved or had no selection.
    SelectionText {
        /// The selection's text, when a selection existed.
        text: Option<String>,
    },
    /// A selection drag in a pane finished, and its text is ready for the
    /// clipboard. The text is never empty.
    SelectionCopied {
        /// The selected text.
        text: String,
    },
    /// A pane closed, whether by shell exit or by `KillPane`.
    PaneClosed {
        /// The pane that closed.
        pane: PaneId,
        /// Why the pane closed.
        reason: CloseReason,
    },
}

/// Every live pane, the layout tree they tile, and the events they have
/// generated since the last drain.
pub(crate) struct Backend {
    factory: Box<dyn PaneFactory>,
    panes: HashMap<PaneId, Pane>,
    tree: LayoutTree,
    geometry: Option<Geometry>,
    /// Whether the primary window has keyboard focus, as the GUI last
    /// reported it.
    window_focused: bool,
    next_pane_id: u32,
    processed: CommandSeq,
    /// The wheel-routing policy handed to every pane's terminal.
    wheel: WheelConfig,
    /// Events generated since the last drain, in generation order.
    outbox: Vec<OrzmuxEvent>,
}

impl Backend {
    /// A backend with no panes and no geometry.
    pub fn new(factory: Box<dyn PaneFactory>, wheel: WheelConfig) -> Self {
        Self {
            factory,
            panes: HashMap::new(),
            tree: LayoutTree::new(),
            geometry: None,
            window_focused: true,
            next_pane_id: 1,
            processed: CommandSeq::default(),
            wheel,
            outbox: Vec::new(),
        }
    }

    /// Pumps one pane and forwards its output, pumping again up to
    /// `PUMP_ROUNDS` times while chunks remain queued; closes the pane on
    /// `ChildExit`.
    pub fn pump_pane(&mut self, id: PaneId) {
        for _ in 0..PUMP_ROUNDS {
            let Some(pane) = self.panes.get_mut(&id) else {
                return;
            };
            let output = pane.tty.pump();
            let more_pending = output.more_pending;
            if let Some(code) = self.forward_items(None, id, output.items) {
                self.close_pane(id, CloseReason::ChildExit { code });
                return;
            }
            if !more_pending {
                return;
            }
        }
    }

    /// Pumps every pane whose next deadline has passed.
    pub fn service_deadlines(&mut self) {
        let now = Instant::now();
        let due: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, p)| p.tty.next_deadline(now).is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.pump_pane(id);
        }
    }

    /// Applies a new window size and republishes the layout.
    pub fn resize(&mut self, size: GridSize, cell_px: CellPixels) {
        self.geometry = Some(Geometry { size, cell_px });
        self.publish_layout();
    }

    /// Spawns a pane at `at` and announces it with `PaneOpened`.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::NoGeometry`] before the window has
    /// reported its size, [`OrzmuxError::UnresolvedTarget`] when the
    /// split target is gone, [`OrzmuxError::RootOccupied`] or
    /// [`OrzmuxError::SplitRefused`] when the tree refuses the
    /// insertion, [`OrzmuxError::Unsolved`] or [`OrzmuxError::Vt`] when
    /// the solved layout gives the new pane no valid rectangle, and
    /// whatever the pane factory returns when the shell will not start.
    /// The tree is left as it was in every case.
    pub fn open_pane(
        &mut self,
        request: RequestId,
        at: NewPaneAt,
        cwd: Option<PathBuf>,
        env: Vec<(EnvKey, EnvValue)>,
    ) -> OrzmuxResult {
        let geometry = self.geometry.ok_or(OrzmuxError::NoGeometry)?;
        let at = self.pinned_pane_at(at)?;
        let new = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let previous_active = self.tree.active();
        let split_target = self.insert_pane(new, at, geometry.size)?;
        let spawn_cwd = cwd.or_else(|| {
            split_target
                .and_then(|id| self.panes.get(&id))
                .and_then(Pane::cwd)
        });
        match self.spawn_pane(new, geometry, spawn_cwd.clone(), env) {
            Ok((tty, size)) => {
                self.panes.insert(
                    new,
                    Pane::new(tty, (size.cols, size.rows, geometry.cell_px), spawn_cwd),
                );
                self.emit(OrzmuxEvent::PaneOpened { pane: new, request });
                self.publish_layout();
                Ok(())
            }
            Err(error) => {
                self.tree.remove(new);
                if let Some(previous) = previous_active {
                    self.tree.select(previous);
                }
                Err(error)
            }
        }
    }

    /// Kills the pane `target` names.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when the target names
    /// no live pane; no pane closes.
    pub fn kill_pane(&mut self, target: PaneTarget) -> OrzmuxResult {
        let id = self.pane_id(target)?;
        self.close_pane(id, CloseReason::Killed);
        Ok(())
    }

    /// Makes `pane` active. A layout is published either way.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`; the active pane is unchanged and the published
    /// layout reflects that.
    pub fn select_pane(&mut self, pane: PaneId) -> OrzmuxResult {
        let selected = self.tree.select(pane);
        self.publish_layout();
        if selected {
            Ok(())
        } else {
            Err(OrzmuxError::UnresolvedTarget)
        }
    }

    /// Moves the active pane one step in `direction`, publishing a
    /// layout only when the active pane moved.
    pub fn select_pane_direction(&mut self, direction: PaneDirection) {
        let moved = self
            .geometry
            .is_some_and(|geometry| self.tree.select_direction(direction, geometry.size));
        if moved {
            self.publish_layout();
        }
    }

    /// Records the window's keyboard focus and reports it to the panes.
    pub fn window_focus(&mut self, focused: bool) {
        self.window_focused = focused;
        self.refresh_focus();
    }

    /// Moves a divider, publishing a layout only when the tree changed.
    pub fn resize_split(&mut self, split: SplitId, position: u16) {
        if self
            .geometry
            .is_some_and(|g| self.tree.resize_split(split, position, g.size))
        {
            self.publish_layout();
        }
    }

    /// Moves one divider of the active pane `cells` cells in `direction`,
    /// publishing a layout only when the tree changed.
    pub fn resize_pane_direction(&mut self, direction: PaneDirection, cells: u16) {
        if self
            .geometry
            .is_some_and(|g| self.tree.resize_direction(direction, cells, g.size))
        {
            self.publish_layout();
        }
    }

    /// Sends a key to the pane `target` names.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when the target names
    /// no live pane, and [`OrzmuxError::PtyWrite`] when its PTY refuses
    /// the write.
    pub fn key_input(
        &mut self,
        target: PaneTarget,
        key: TerminalKey,
        mods: TerminalModifiers,
    ) -> OrzmuxResult {
        let id = self.pane_id(target)?;
        self.write_pty(id, |tty| tty.send_key(&key, &mods))
    }

    /// Sends pasted text to the pane `target` names.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when the target names
    /// no live pane, and [`OrzmuxError::PtyWrite`] when its PTY refuses
    /// the write.
    pub fn paste(&mut self, target: PaneTarget, text: String) -> OrzmuxResult {
        let id = self.pane_id(target)?;
        self.write_pty(id, |tty| tty.send_paste(&text))
    }

    /// Routes a wheel event to `pane` under the backend's wheel policy.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`, and [`OrzmuxError::PtyWrite`] when its PTY
    /// refuses the write.
    pub fn wheel(&mut self, pane: PaneId, input: WheelInput) -> OrzmuxResult {
        let wheel = self.wheel;
        self.write_pty(pane, |tty| tty.send_wheel(input, &wheel))
    }

    /// Routes a pointer event to `pane` by that pane's live VT modes, and
    /// emits [`OrzmuxEvent::SelectionCopied`] when the event finished a
    /// selection drag.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`, and [`OrzmuxError::PtyWrite`] when its PTY
    /// refuses the reports.
    pub fn pointer(&mut self, pane: PaneId, input: PointerInput) -> OrzmuxResult {
        if let Some(text) = self.write_pty(pane, |tty| tty.send_pointer(input))? {
            self.emit(OrzmuxEvent::SelectionCopied { text });
        }
        Ok(())
    }

    /// Scrolls `pane`'s viewport.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`.
    pub fn scroll(&mut self, pane: PaneId, scroll: Scroll) -> OrzmuxResult {
        self.pane_mut(pane)?.tty.scroll(scroll);
        Ok(())
    }

    /// Drops `pane`'s selection.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`.
    pub fn selection_clear(&mut self, pane: PaneId) -> OrzmuxResult {
        self.pane_mut(pane)?.tty.clear_selection();
        Ok(())
    }

    /// Answers with the selected text of the pane `target` names. The
    /// answer is `None` when the target does not resolve or holds no
    /// selection.
    pub fn copy_selection(&mut self, target: PaneTarget) {
        let text = self
            .pane_id(target)
            .ok()
            .and_then(|id| self.panes.get(&id))
            .and_then(|p| p.tty.vt().selection_text())
            .filter(|t| !t.is_empty());
        self.emit(OrzmuxEvent::SelectionText { text });
    }

    /// Removes the named placements from `pane`.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`.
    pub fn remove_placements(&mut self, pane: PaneId, instances: Vec<InstanceId>) -> OrzmuxResult {
        self.pane_mut(pane)?.tty.remove_placements(&instances);
        Ok(())
    }

    /// Mounts a placement in `pane` at the given row and column.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `pane`.
    pub fn mount_placement(
        &mut self,
        pane: PaneId,
        instance: InstanceId,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
    ) -> OrzmuxResult {
        self.pane_mut(pane)?
            .tty
            .mount_placement_at(instance, row, column, size);
        Ok(())
    }

    /// Answers a `NewPane` request with the failure that refused it.
    // NOTE: `OrzmuxEvent` derives `Clone` and `PartialEq`, which
    // `OrzmaTtyError` does not, so the wire carries the rendered text
    // rather than the error itself.
    pub fn fail_spawn(&mut self, request: RequestId, error: &OrzmuxError) {
        self.emit(OrzmuxEvent::SpawnFailed {
            request,
            error: error.to_string(),
        });
    }

    /// Empties the outbox, yielding the events in generation order.
    pub fn drain_events(&mut self) -> impl Iterator<Item = OrzmuxEvent> + '_ {
        self.outbox.drain(..)
    }

    /// Every live pane's readable streams, in no fixed order.
    pub fn readiness(&self) -> impl Iterator<Item = (PaneId, Readiness<'_>)> {
        self.panes
            .iter()
            .map(|(id, pane)| (*id, pane.tty.readiness()))
    }

    /// The earliest deadline any pane wants to be pumped at.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        self.panes
            .values()
            .filter_map(|p| p.tty.next_deadline(now))
            .min()
    }

    /// Every live pane's unread chunk count, in no fixed order.
    pub fn chunk_depths(&self) -> impl Iterator<Item = (PaneId, ChunkDepth)> {
        self.panes
            .iter()
            .map(|(id, pane)| (*id, ChunkDepth(pane.tty.pending_chunk_count())))
    }

    /// Records the command watermark the next published layout carries.
    pub fn set_processed(&mut self, seq: CommandSeq) {
        self.processed = seq;
    }

    /// The pane layout tree.
    #[cfg(test)]
    pub fn tree(&self) -> &LayoutTree {
        &self.tree
    }

    /// The live pane `id` names, or `None` when no pane carries it.
    #[cfg(test)]
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    /// The id the next spawned pane takes.
    #[cfg(test)]
    pub fn next_pane_id(&self) -> u32 {
        self.next_pane_id
    }

    /// Every live pane, in no fixed order.
    #[cfg(test)]
    pub fn panes(&self) -> impl Iterator<Item = (PaneId, &Pane)> {
        self.panes.iter().map(|(id, pane)| (*id, pane))
    }

    /// `at` with its split target pinned to a concrete, live pane.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when the split target
    /// names no live pane.
    fn pinned_pane_at(&self, at: NewPaneAt) -> OrzmuxResult<PinnedPaneAt> {
        let NewPaneAt::Split { pane, orientation } = at else {
            return Ok(PinnedPaneAt::Root);
        };
        let pane = self.pane_id(pane)?;
        // NOTE: `pane` is pinned to this concrete id now rather than
        // re-resolved later. `NewPaneAt::Split` can carry
        // `PaneTarget::Active`, and re-resolving it after something else
        // moved the active pane would divide whichever pane is active
        // then, not the one the command named.
        Ok(PinnedPaneAt::Split { pane, orientation })
    }

    /// Inserts `new` into the tree at `at`. Returns the pane a split
    /// divides, or `None` for a root pane.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::RootOccupied`] when a root pane is
    /// requested while the tree holds one, and
    /// [`OrzmuxError::SplitRefused`] when the target has too little
    /// room to divide.
    fn insert_pane(
        &mut self,
        new: PaneId,
        at: PinnedPaneAt,
        window: GridSize,
    ) -> OrzmuxResult<Option<PaneId>> {
        match at {
            PinnedPaneAt::Root => {
                self.tree.insert_root(new)?;
                Ok(None)
            }
            PinnedPaneAt::Split { pane, orientation } => {
                self.tree.split(pane, orientation, new, window)?;
                Ok(Some(pane))
            }
        }
    }

    /// Spawns the terminal for `new` at the size the solved layout gives
    /// it.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::Unsolved`] when the tree does not place
    /// the pane, [`OrzmuxError::Vt`] when its rectangle is not a valid
    /// size, and [`OrzmuxError::SpawnShell`] when the shell refuses to
    /// start.
    fn spawn_pane(
        &mut self,
        new: PaneId,
        geometry: Geometry,
        cwd: Option<PathBuf>,
        env: Vec<(EnvKey, EnvValue)>,
    ) -> OrzmuxResult<(OrzmaTty<OrzmaVt>, GridSize)> {
        let rect = self
            .tree
            .solve(geometry.size)
            .rect_of(new)
            .ok_or(OrzmuxError::Unsolved)?;
        let size = GridSize::new(rect.cols, rect.rows)?;
        let tty = self.factory.spawn(size, geometry.cell_px, cwd, env)?;
        Ok((tty, size))
    }

    /// Tells every pane whether it holds focus, then re-solves the tree,
    /// resizes every pane whose applied geometry differs, flushes those
    /// panes, and emits their signals followed by one `Layout` carrying
    /// their frames. Everything after the focus update is a no-op without
    /// geometry. A pane whose resize is refused keeps its old size while
    /// the remaining panes are still resized and the `Layout` still
    /// publishes.
    fn publish_layout(&mut self) {
        self.refresh_focus();
        let Some(geometry) = self.geometry else {
            return;
        };
        let solved = self.tree.solve(geometry.size);
        let mut frames: Vec<(PaneId, Frame)> = Vec::new();
        for rect in &solved.panes {
            let Some(pane) = self.panes.get_mut(&rect.pane) else {
                continue;
            };
            let wanted = (rect.cols, rect.rows, geometry.cell_px);
            if pane.applied == wanted {
                continue;
            }
            let size = match GridSize::new(rect.cols, rect.rows) {
                Ok(size) => size,
                Err(err) => {
                    tracing::warn!(pane = ?rect.pane, %err, "pane rect is not a valid grid size; keeping the old size");
                    continue;
                }
            };
            match pane.tty.resize(size, geometry.cell_px) {
                Ok(()) => pane.applied = wanted,
                Err(err) => {
                    tracing::warn!(pane = ?rect.pane, %err, "pane resize failed; keeping the old size");
                    continue;
                }
            }
            let flushed = pane.tty.flush_now();
            self.forward_items(Some(&mut frames), rect.pane, flushed.items);
        }
        let layout = Layout {
            seq: self.processed,
            size: solved.size,
            active: self.tree.active(),
            panes: solved.panes,
            separators: solved.separators,
        };
        self.emit(OrzmuxEvent::Layout { layout, frames });
    }

    /// Tells every pane whether it holds focus: the active pane does while
    /// the window is focused, and no pane does otherwise. A report a pane
    /// refuses is logged without stopping the others, and reports to
    /// different panes reach their PTYs in no fixed order.
    fn refresh_focus(&mut self) {
        let target = self.tree.active().filter(|_| self.window_focused);
        let report = |id: PaneId, result: OrzmaTtyResult| {
            if let Err(err) = result {
                log_refused_write(id, "focus report", &err, Level::WARN);
            }
        };
        for (id, pane) in self.panes.iter_mut().filter(|(id, _)| Some(**id) != target) {
            report(*id, pane.tty.set_focused(false));
        }
        if let Some(id) = target
            && let Some(pane) = self.panes.get_mut(&id)
        {
            report(id, pane.tty.set_focused(true));
        }
    }

    /// Forwards a pump's items in order: each signal as a `Signal` event,
    /// each frame as a `Frame` event, or into `layout_frames` when the
    /// caller publishes the frames itself. Returns `Some(code)` when the
    /// items carried `ChildExit`.
    fn forward_items(
        &mut self,
        mut layout_frames: Option<&mut Vec<(PaneId, Frame)>>,
        id: PaneId,
        items: Vec<PumpItem>,
    ) -> Option<Option<i32>> {
        let mut exited = None;
        for item in items {
            match item {
                PumpItem::Signal(TtySignal::ChildExit { code }) => exited = Some(code),
                PumpItem::Signal(TtySignal::Vt(signal)) => {
                    if let VtSignal::CurrentDir(path) = &signal
                        && let Some(pane) = self.panes.get_mut(&id)
                    {
                        pane.set_reported_cwd(path.clone());
                    }
                    self.emit(OrzmuxEvent::Signal { pane: id, signal });
                }
                PumpItem::Frame(frame) => match layout_frames.as_deref_mut() {
                    Some(frames) => frames.push((id, frame)),
                    None => self.emit(OrzmuxEvent::Frame { pane: id, frame }),
                },
            }
        }
        exited
    }

    /// Removes a pane from the tree and the pool after flushing its last
    /// output, then publishes the layout the survivors get.
    fn close_pane(&mut self, id: PaneId, reason: CloseReason) {
        if let Some(pane) = self.panes.get_mut(&id) {
            let flushed = pane.tty.flush_now();
            self.forward_items(None, id, flushed.items);
        }
        self.tree.remove(id);
        self.panes.remove(&id);
        self.emit(OrzmuxEvent::PaneClosed { pane: id, reason });
        self.publish_layout();
    }

    /// The id of the pane `target` names.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] for an id no live pane
    /// carries, and for [`PaneTarget::Active`] while no pane is active.
    fn pane_id(&self, target: PaneTarget) -> OrzmuxResult<PaneId> {
        let id = match target {
            PaneTarget::Active => self.tree.active(),
            PaneTarget::Id(id) => self.panes.contains_key(&id).then_some(id),
        };
        id.ok_or(OrzmuxError::UnresolvedTarget)
    }

    /// The mutable state of the pane `id` carries.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `id`.
    fn pane_mut(&mut self, id: PaneId) -> OrzmuxResult<&mut Pane> {
        self.panes.get_mut(&id).ok_or(OrzmuxError::UnresolvedTarget)
    }

    /// Runs `write` against the PTY of the pane `id` names, returning what
    /// it returns.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when no live pane
    /// carries `id`, and [`OrzmuxError::PtyWrite`] when the PTY refuses
    /// the write.
    fn write_pty<T>(
        &mut self,
        id: PaneId,
        write: impl FnOnce(&mut OrzmaTty<OrzmaVt>) -> OrzmaTtyResult<T>,
    ) -> OrzmuxResult<T> {
        write(&mut self.pane_mut(id)?.tty)
            .map_err(|source| OrzmuxError::PtyWrite { pane: id, source })
    }

    fn emit(&mut self, event: OrzmuxEvent) {
        self.outbox.push(event);
    }
}

/// Logs a PTY write the pane's terminal refused.
///
/// The first rejection of a stuck episode warns and later rejections stay
/// at debug, a closed writer logs at debug, and any other failure logs at
/// `failure`, which is `ERROR` or `WARN`.
pub fn log_refused_write(pane: PaneId, what: &'static str, err: &OrzmaTtyError, failure: Level) {
    match err {
        OrzmaTtyError::PtyWriteQueueFull {
            dropped_in_episode: 1,
        } => tracing::warn!(?pane, %err, "{what} dropped: it does not fit in the PTY input queue"),
        OrzmaTtyError::PtyWriteQueueFull { .. } | OrzmaTtyError::PtyWriterClosed => {
            tracing::debug!(?pane, %err, "{what} dropped");
        }
        _ if failure == Level::ERROR => {
            tracing::error!(?pane, ?err, "{what} dropped: an earlier PTY write failed");
        }
        _ => tracing::warn!(?pane, ?err, "{what} dropped: an earlier PTY write failed"),
    }
}

/// The window geometry the GUI last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geometry {
    size: GridSize,
    cell_px: CellPixels,
}

/// Where a new pane goes, with its split target already pinned to a
/// live pane.
#[derive(Clone, Copy)]
enum PinnedPaneAt {
    /// The first pane; valid only while the tree is empty.
    Root,
    /// Split `pane`, putting the new pane right of / below it.
    Split {
        /// The pane being split.
        pane: PaneId,
        /// The direction of the divider the split introduces.
        orientation: SplitOrientation,
    },
}

/// How many times one wake pumps the same pane while its chunks stay
/// queued, before other panes and the command channel get a turn.
const PUMP_ROUNDS: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that request ids mint strictly increasing values.
    ///
    /// Case: the GUI fires two pane spawns in one frame and must tell
    /// the two `PaneOpened` answers apart.
    #[test]
    fn request_ids_are_strictly_increasing() {
        let a = RequestId::next();
        let b = RequestId::next();
        assert!(a.0 < b.0);
    }

    /// Asserts that the default layout has no panes, no separators, and
    /// no active pane.
    ///
    /// Case: the GUI holds a layout of its own before the backend opens
    /// the first pane.
    #[test]
    fn the_default_layout_has_no_panes() {
        let layout = Layout::default();
        assert!(layout.panes.is_empty());
        assert!(layout.separators.is_empty());
        assert_eq!(layout.active, None);
    }
}
