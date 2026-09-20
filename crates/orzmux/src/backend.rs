//! The multiplexer backend loop: owns every pane, waits on the command
//! channel and each pane's PTY streams with one `Select`, and emits
//! layout / frame / signal events to the GUI.

use crate::backend::pane::{Pane, PaneFactory};
use crate::backend::queue_sample::{ChunkDepth, QueueSampler};
use crate::layout::LayoutTree;
use crate::protocol::{
    CloseReason, CommandSeq, Layout, NewPaneAt, OrzmuxCommand, OrzmuxEvent, PaneId, PaneTarget,
    RequestId,
};
use crossbeam_channel::{Receiver, Select, Sender, TryRecvError};
use orzma_tty::CellPixels;
use orzma_tty::prelude::{OrzmaTtyError, OrzmaTtyResult, PumpOutput, TtySignal, WheelConfig};
use orzma_vt::prelude::{Frame, GridSize, Vt, VtSignal};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tracing::Level;

pub(crate) mod pane;
pub(crate) mod queue_sample;
pub(crate) use pane::ShellFactory;

/// The backend state, driven by [`Backend::run`] on its own thread.
pub(crate) struct Backend {
    factory: Box<dyn PaneFactory>,
    panes: HashMap<PaneId, Pane>,
    tree: LayoutTree,
    geometry: Option<Geometry>,
    /// Whether the primary window has keyboard focus, as the GUI last
    /// reported it.
    window_focused: bool,
    commands: Receiver<(CommandSeq, OrzmuxCommand)>,
    events: Sender<OrzmuxEvent>,
    next_pane_id: u32,
    processed: CommandSeq,
    /// Set when the GUI's event receiver is gone; the loop exits.
    gui_gone: bool,
    /// What each `Select` index of the last `wait_ready` referred to.
    sources: Vec<Ready>,
    /// Per-queue peaks between samples; logged once a second.
    sampler: QueueSampler,
    /// The wheel-routing policy handed to every pane's terminal.
    wheel: WheelConfig,
}

impl Backend {
    /// A backend with no panes and no geometry.
    pub(crate) fn new(
        factory: Box<dyn PaneFactory>,
        commands: Receiver<(CommandSeq, OrzmuxCommand)>,
        events: Sender<OrzmuxEvent>,
        wheel: WheelConfig,
    ) -> Self {
        Self {
            factory,
            panes: HashMap::new(),
            tree: LayoutTree::new(),
            geometry: None,
            window_focused: true,
            commands,
            events,
            next_pane_id: 1,
            processed: CommandSeq::default(),
            gui_gone: false,
            sources: Vec::new(),
            sampler: QueueSampler::new(Instant::now()),
            wheel,
        }
    }

    /// Runs until the command channel disconnects (the GUI dropped its
    /// client) or the GUI stops receiving events. Dropping the panes on
    /// return kills every child.
    pub(crate) fn run(mut self) {
        loop {
            let ready = self.wait_ready();
            self.record_queue_depths();
            match ready {
                Some(Ready::Commands) if !self.drain_commands() => return,
                Some(Ready::Commands) => {}
                Some(Ready::Pane(pane)) => self.pump_pane(pane),
                None => {}
            }
            self.service_deadlines();
            self.report_queue_sample(Instant::now());
            if self.gui_gone {
                return;
            }
        }
    }

    /// Applies one command. Unknown panes and an unresolvable `Active`
    /// are dropped with a debug log; `CopySelection` always answers,
    /// `SelectPane` always publishes a layout, and `SelectPaneDirection`
    /// publishes one only when the active pane moved.
    pub(crate) fn handle_command(&mut self, seq: CommandSeq, command: OrzmuxCommand) {
        self.processed = seq;
        match command {
            OrzmuxCommand::Resize { size, cell_px } => self.on_resize(size, cell_px),
            OrzmuxCommand::NewPane {
                request,
                at,
                cwd,
                env,
            } => self.on_new_pane(request, at, cwd, env),
            OrzmuxCommand::KillPane { pane } => {
                if let Some(id) = self.resolve_or_log(pane, "KillPane") {
                    self.close_pane(id, CloseReason::Killed);
                }
            }
            OrzmuxCommand::SelectPane { pane } => {
                if !self.tree.select(pane) {
                    tracing::debug!(?pane, "select of an unknown pane refused");
                }
                self.publish_layout();
            }
            OrzmuxCommand::SelectPaneDirection { direction } => {
                let moved = self
                    .geometry
                    .is_some_and(|geometry| self.tree.select_direction(direction, geometry.size));
                if moved {
                    self.publish_layout();
                }
            }
            OrzmuxCommand::WindowFocus { focused } => {
                self.window_focused = focused;
                self.refresh_focus();
            }
            OrzmuxCommand::ResizeSplit { split, position } => {
                if self
                    .geometry
                    .is_some_and(|g| self.tree.resize_split(split, position, g.size))
                {
                    self.publish_layout();
                }
            }
            OrzmuxCommand::KeyInput { pane, key, mods } => {
                if let Some(id) = self.resolve_or_log(pane, "KeyInput")
                    && let Some(p) = self.panes.get_mut(&id)
                    && let Err(err) = p.tty.send_key(&key, &mods)
                {
                    log_refused_write(id, "key", &err, Level::ERROR);
                }
            }
            OrzmuxCommand::Paste { pane, text } => {
                if let Some(id) = self.resolve_or_log(pane, "Paste")
                    && let Some(p) = self.panes.get_mut(&id)
                    && let Err(err) = p.tty.send_paste(&text)
                {
                    log_refused_write(id, "paste", &err, Level::ERROR);
                }
            }
            OrzmuxCommand::MouseInput { pane, report } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "MouseInput")
                    && let Err(err) = p.tty.send_mouse(report)
                {
                    log_refused_write(pane, "mouse report", &err, Level::ERROR);
                }
            }
            OrzmuxCommand::Wheel { pane, input } => {
                let wheel = self.wheel;
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "Wheel")
                    && let Err(err) = p.tty.send_wheel(input, &wheel)
                {
                    log_refused_write(pane, "wheel", &err, Level::ERROR);
                }
            }
            OrzmuxCommand::Scroll { pane, scroll } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "Scroll") {
                    p.tty.scroll(scroll);
                }
            }
            OrzmuxCommand::SelectionStart {
                pane,
                cell,
                side,
                kind,
            } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "SelectionStart") {
                    p.tty.start_selection(cell, side, kind);
                }
            }
            OrzmuxCommand::SelectionUpdate { pane, cell, side } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "SelectionUpdate") {
                    p.tty.extend_selection(cell, side);
                }
            }
            OrzmuxCommand::SelectionClear { pane } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "SelectionClear") {
                    p.tty.clear_selection();
                }
            }
            OrzmuxCommand::CopySelection { pane } => {
                let text = self
                    .resolve(pane)
                    .and_then(|id| self.panes.get(&id))
                    .and_then(|p| p.tty.vt().selection_text())
                    .filter(|t| !t.is_empty());
                self.emit(OrzmuxEvent::SelectionText { text });
            }
            OrzmuxCommand::RemovePlacements { pane, instances } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "RemovePlacements") {
                    p.tty.remove_placements(&instances);
                }
            }
            OrzmuxCommand::MountPlacement {
                pane,
                instance,
                row,
                column,
                size,
            } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "MountPlacement") {
                    p.tty.mount_placement_at(instance, row, column, size);
                }
            }
        }
    }

    /// Pumps one pane and forwards its output, pumping again up to
    /// `PUMP_ROUNDS` times while chunks remain queued; closes the pane on
    /// `ChildExit`.
    pub(crate) fn pump_pane(&mut self, id: PaneId) {
        for _ in 0..PUMP_ROUNDS {
            let Some(pane) = self.panes.get_mut(&id) else {
                return;
            };
            let output = pane.tty.pump();
            let more_pending = output.more_pending;
            if let Some(code) = self.emit_pump_output(id, output) {
                self.close_pane(id, CloseReason::ChildExit { code });
                return;
            }
            if !more_pending {
                return;
            }
        }
    }

    /// Pumps every pane whose coalescer deadline has passed.
    pub(crate) fn service_deadlines(&mut self) {
        let now = Instant::now();
        let due: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, p)| p.tty.next_deadline().is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.pump_pane(id);
        }
    }

    /// Blocks until a command or a pane stream is ready, or the earliest
    /// of the coalescer deadlines and the sampler's report deadline
    /// passes. Returns the ready source, `None` on timeout.
    fn wait_ready(&mut self) -> Option<Ready> {
        let mut select = Select::new();
        self.sources.clear();
        select.recv(&self.commands);
        self.sources.push(Ready::Commands);
        for (id, pane) in &self.panes {
            let readiness = pane.tty.readiness();
            select.recv(readiness.chunks);
            self.sources.push(Ready::Pane(*id));
            if let Some(exit) = readiness.exit {
                select.recv(exit);
                self.sources.push(Ready::Pane(*id));
            }
        }
        let index = match self.next_wake_deadline() {
            Some(deadline) => select.ready_deadline(deadline).ok()?,
            None => select.ready(),
        };
        Some(self.sources[index])
    }

    /// The earliest of the coalescer deadlines and the sampler's report
    /// deadline, or `None` when every pane is idle and no peak waits to
    /// be reported.
    fn next_wake_deadline(&self) -> Option<Instant> {
        self.panes
            .values()
            .filter_map(|p| p.tty.next_deadline())
            .chain(self.sampler.report_deadline())
            .min()
    }

    /// Applies up to `COMMAND_BATCH` queued commands. Returns `false`
    /// when the command channel is disconnected.
    fn drain_commands(&mut self) -> bool {
        for _ in 0..COMMAND_BATCH {
            match self.commands.try_recv() {
                Ok((seq, command)) => self.handle_command(seq, command),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        true
    }

    fn on_resize(&mut self, size: GridSize, cell_px: CellPixels) {
        self.geometry = Some(Geometry { size, cell_px });
        self.publish_layout();
    }

    fn on_new_pane(
        &mut self,
        request: RequestId,
        at: NewPaneAt,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
    ) {
        let Some(geometry) = self.geometry else {
            self.emit(OrzmuxEvent::SpawnFailed {
                request,
                error: "no geometry".to_string(),
            });
            return;
        };
        let new = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let previous_active = self.tree.active();
        let split_target = match self.insert_pane(new, at, geometry.size) {
            Ok(split_target) => split_target,
            Err(error) => {
                self.emit(OrzmuxEvent::SpawnFailed {
                    request,
                    error: error.to_string(),
                });
                return;
            }
        };
        let spawn_cwd = cwd.or_else(|| {
            split_target
                .and_then(|id| self.panes.get(&id))
                .and_then(Pane::cwd)
        });
        let spawned = self
            .tree
            .solve(geometry.size)
            .rect_of(new)
            .ok_or_else(|| "the new pane is not in the solved layout".to_string())
            .and_then(|rect| GridSize::new(rect.cols, rect.rows).map_err(|err| err.to_string()))
            .and_then(|size| {
                self.factory
                    .spawn(size, geometry.cell_px, spawn_cwd.clone(), env)
                    .map(|tty| (tty, size))
                    .map_err(|err| err.to_string())
            });
        match spawned {
            Ok((tty, size)) => {
                self.panes.insert(
                    new,
                    Pane::new(tty, (size.cols, size.rows, geometry.cell_px), spawn_cwd),
                );
                self.emit(OrzmuxEvent::PaneOpened { pane: new, request });
                self.publish_layout();
            }
            Err(error) => {
                self.tree.remove(new);
                if let Some(previous) = previous_active {
                    self.tree.select(previous);
                }
                self.emit(OrzmuxEvent::SpawnFailed { request, error });
            }
        }
    }

    /// Inserts `new` into the tree at `at`. Returns the pane a split
    /// divides, `None` for a root pane, or why the insertion was refused.
    fn insert_pane(
        &mut self,
        new: PaneId,
        at: NewPaneAt,
        window: GridSize,
    ) -> Result<Option<PaneId>, &'static str> {
        match at {
            NewPaneAt::Root => {
                self.tree
                    .insert_root(new)
                    .map_err(|_| "root already open")?;
                Ok(None)
            }
            NewPaneAt::Split { pane, orientation } => {
                let target = self.resolve(pane).ok_or("no target pane")?;
                self.tree
                    .split(target, orientation, new, window)
                    .map_err(|_| "no space")?;
                Ok(Some(target))
            }
        }
    }

    /// Tells every pane whether it holds focus, then re-solves the tree,
    /// resizes every pane whose applied geometry differs, flushes those
    /// panes, and emits their signals followed by one `Layout` carrying
    /// their frames. Everything after the focus update is a no-op without
    /// geometry.
    fn publish_layout(&mut self) {
        self.refresh_focus();
        let Some(geometry) = self.geometry else {
            return;
        };
        let solved = self.tree.solve(geometry.size);
        let mut frames: Vec<(PaneId, Frame)> = Vec::new();
        let mut signals: Vec<(PaneId, VtSignal)> = Vec::new();
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
            for signal in flushed.signals {
                if let TtySignal::Vt(signal) = signal {
                    signals.push((rect.pane, signal));
                }
            }
            if let Some(frame) = flushed.frame {
                frames.push((rect.pane, frame));
            }
        }
        for (pane, signal) in signals {
            self.emit(OrzmuxEvent::Signal { pane, signal });
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

    /// Forwards a pump's signals and frame, in that order.
    /// Returns `Some(code)` when the signals carried `ChildExit`.
    fn emit_pump_output(&mut self, id: PaneId, output: PumpOutput) -> Option<Option<i32>> {
        let mut exited = None;
        for signal in output.signals {
            match signal {
                TtySignal::ChildExit { code } => exited = Some(code),
                TtySignal::Vt(signal) => {
                    if let VtSignal::CurrentDir(path) = &signal
                        && let Some(pane) = self.panes.get_mut(&id)
                    {
                        pane.set_reported_cwd(path.clone());
                    }
                    self.emit(OrzmuxEvent::Signal { pane: id, signal });
                }
            }
        }
        if let Some(frame) = output.frame {
            self.emit(OrzmuxEvent::Frame { pane: id, frame });
        }
        exited
    }

    /// Removes a pane from the tree and the pool after flushing its last
    /// output, then publishes the layout the survivors get.
    fn close_pane(&mut self, id: PaneId, reason: CloseReason) {
        if let Some(pane) = self.panes.get_mut(&id) {
            let flushed = pane.tty.flush_now();
            self.emit_pump_output(id, flushed);
        }
        self.tree.remove(id);
        self.panes.remove(&id);
        self.emit(OrzmuxEvent::PaneClosed { pane: id, reason });
        self.publish_layout();
    }

    /// Resolves a target against the active pane.
    fn resolve(&self, target: PaneTarget) -> Option<PaneId> {
        match target {
            PaneTarget::Active => self.tree.active(),
            PaneTarget::Id(id) => self.panes.contains_key(&id).then_some(id),
        }
    }

    /// Resolves a target, logging a debug line naming `command` when it
    /// does not resolve (an unknown `PaneId`, or `Active` with no active
    /// pane).
    fn resolve_or_log(&self, target: PaneTarget, command: &'static str) -> Option<PaneId> {
        let resolved = self.resolve(target);
        if resolved.is_none() {
            tracing::debug!(?target, command, "pane command dropped: no such pane");
        }
        resolved
    }

    /// Resolves a target to its pane's mutable state, logging a debug
    /// line naming `command` when it does not resolve.
    fn pane_mut(&mut self, target: PaneTarget, command: &'static str) -> Option<&mut Pane> {
        let id = self.resolve_or_log(target, command)?;
        self.panes.get_mut(&id)
    }

    fn emit(&mut self, event: OrzmuxEvent) {
        if self.events.send(event).is_err() {
            self.gui_gone = true;
        }
    }

    /// Records every queue's current depth into the sampler: each pane's
    /// unread chunk count, the event channel, and the command channel.
    fn record_queue_depths(&mut self) {
        for (id, pane) in &self.panes {
            self.sampler
                .record_pane_depth(*id, ChunkDepth(pane.tty.pending_chunk_count()));
        }
        self.sampler
            .record_channel_depths(self.events.len(), self.commands.len());
    }

    /// Logs the sample the sampler hands out at `now`, if one is due:
    /// one line per pane with a recorded chunk peak and one line for
    /// the channels when either has a recorded peak.
    fn report_queue_sample(&mut self, now: Instant) {
        let Some(sample) = self.sampler.sample(now) else {
            return;
        };
        for (pane, depth) in sample.chunks {
            tracing::debug!(
                target: "orzmux::queues",
                ?pane,
                depth = depth.0,
                "chunk queue peak"
            );
        }
        if sample.events > 0 || sample.commands > 0 {
            tracing::debug!(
                target: "orzmux::queues",
                events = sample.events,
                commands = sample.commands,
                "event and command queue peaks"
            );
        }
    }
}

/// The window geometry the GUI last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geometry {
    size: GridSize,
    cell_px: CellPixels,
}

/// What one ready `Select` index refers to.
#[derive(Debug, Clone, Copy)]
enum Ready {
    Commands,
    Pane(PaneId),
}

/// Logs a PTY write the pane's terminal refused.
///
/// The first rejection of a stuck episode warns and later rejections stay
/// at debug, a closed writer logs at debug, and any other failure logs at
/// `failure`, which is `ERROR` or `WARN`.
fn log_refused_write(pane: PaneId, what: &'static str, err: &OrzmaTtyError, failure: Level) {
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

/// How many queued commands one iteration applies before pumping panes.
const COMMAND_BATCH: usize = 64;

/// How many times one wake pumps the same pane while its chunks stay
/// queued, before other panes and the command channel get a turn.
const PUMP_ROUNDS: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::{PaneDirection, SplitId, SplitOrientation};
    use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
    use orzma_tty::prelude::{
        CellCoord, KeyText, OrzmaTty, OrzmaTtyError, OrzmaTtyResult, ProtocolModifiers,
        TerminalKey, TerminalModifiers, WheelInput, WheelModifiers,
    };
    use orzma_tty::test_support::{BlockingSink, CaptureSink, FailingSink};
    use orzma_vt::prelude::OrzmaVt;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    /// The test's ends of one spawned pane's streams.
    struct FakePane {
        chunk_tx: Sender<Vec<u8>>,
        exit_tx: Sender<Option<i32>>,
        sink: CaptureSink,
    }

    /// What the factory recorded, shared with the harness through `Arc`.
    #[derive(Default)]
    struct FactoryLog {
        fail_next: AtomicBool,
        /// Makes the next spawned pane's PTY writer fail every write.
        fail_writes_next: AtomicBool,
        /// When set, the next spawned pane's PTY writer is this sink, whose
        /// writes block until it is released, as if the pane's application
        /// stopped reading stdin.
        block_writes_next: Mutex<Option<BlockingSink>>,
        /// When set, every spawned pane's output stream starts with these
        /// bytes, left unread until the test pumps the pane.
        spawn_output: Mutex<Option<Vec<u8>>>,
        sizes: Mutex<Vec<GridSize>>,
        cwds: Mutex<Vec<Option<PathBuf>>>,
    }

    /// Spawns PTY-less terminals and hands the test their input ends.
    struct FakeFactory {
        spawned: Sender<FakePane>,
        log: Arc<FactoryLog>,
    }

    impl PaneFactory for FakeFactory {
        fn spawn(
            &mut self,
            size: GridSize,
            _cell_px: CellPixels,
            cwd: Option<PathBuf>,
            _env: Vec<(String, String)>,
        ) -> OrzmaTtyResult<OrzmaTty<OrzmaVt>> {
            self.log.sizes.lock().unwrap().push(size);
            self.log.cwds.lock().unwrap().push(cwd);
            if self.log.fail_next.swap(false, Ordering::AcqRel) {
                return Err(OrzmaTtyError::SpawnShell(anyhow::anyhow!("injected")));
            }
            let (chunk_tx, chunk_rx) = unbounded();
            let (exit_tx, exit_rx) = unbounded();
            if let Some(output) = self.log.spawn_output.lock().unwrap().clone() {
                chunk_tx.send(output).unwrap();
            }
            let sink = CaptureSink::default();
            let writer: Box<dyn Write + Send> =
                if self.log.fail_writes_next.swap(false, Ordering::AcqRel) {
                    Box::new(FailingSink)
                } else if let Some(gate) = self.log.block_writes_next.lock().unwrap().take() {
                    Box::new(gate)
                } else {
                    Box::new(sink.clone())
                };
            let tty = OrzmaTty::detached_with_channels(
                OrzmaVt::new(size, 100),
                size,
                writer,
                chunk_rx,
                exit_rx,
            )?;
            let _ = self.spawned.send(FakePane {
                chunk_tx,
                exit_tx,
                sink,
            });
            Ok(tty)
        }
    }

    struct Harness {
        backend: Backend,
        events: Receiver<OrzmuxEvent>,
        panes: Receiver<FakePane>,
        log: Arc<FactoryLog>,
        seq: u64,
        /// Held so the command channel stays connected.
        _commands: Sender<(CommandSeq, OrzmuxCommand)>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_wheel(WheelConfig::default())
        }

        /// A harness whose backend routes the wheel by `wheel`.
        fn with_wheel(wheel: WheelConfig) -> Self {
            let (spawned_tx, spawned_rx) = unbounded();
            let (command_tx, command_rx) = unbounded();
            let (event_tx, event_rx) = unbounded();
            let log = Arc::new(FactoryLog::default());
            let factory = FakeFactory {
                spawned: spawned_tx,
                log: Arc::clone(&log),
            };
            Self {
                backend: Backend::new(Box::new(factory), command_rx, event_tx, wheel),
                events: event_rx,
                panes: spawned_rx,
                log,
                seq: 0,
                _commands: command_tx,
            }
        }

        fn send(&mut self, command: OrzmuxCommand) -> CommandSeq {
            self.seq += 1;
            let seq = CommandSeq(self.seq);
            self.backend.handle_command(seq, command);
            seq
        }

        fn drain(&self) -> VecDeque<OrzmuxEvent> {
            self.events.try_iter().collect()
        }

        /// Waits until every live pane's queued PTY writes have been
        /// written.
        fn settle_writes(&self) {
            for pane in self.backend.panes.values() {
                pane.tty.settle_writes();
            }
        }

        fn resize(&mut self, size: GridSize) {
            self.send(OrzmuxCommand::Resize {
                size,
                cell_px: CellPixels {
                    width: 8,
                    height: 16,
                },
            });
        }

        fn open_root(&mut self) -> (PaneId, FakePane) {
            self.resize(GridSize::new(80, 24).expect("a valid size"));
            self.drain();
            self.send(OrzmuxCommand::NewPane {
                request: RequestId(1),
                at: NewPaneAt::Root,
                cwd: None,
                env: vec![],
            });
            let events = self.drain();
            let Some(OrzmuxEvent::PaneOpened { pane, .. }) = events.front() else {
                panic!("expected PaneOpened, got {events:?}");
            };
            (*pane, self.panes.try_recv().expect("one spawned pane"))
        }
    }

    /// Asserts that `NewPane { Root }` before any `Resize` fails instead
    /// of guessing a size.
    ///
    /// Case: a misordered GUI start-up spawns before the window metrics
    /// exist.
    #[test]
    fn a_root_pane_before_geometry_is_refused() {
        let mut h = Harness::new();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(9),
            at: NewPaneAt::Root,
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        assert!(matches!(
            events.front(),
            Some(OrzmuxEvent::SpawnFailed {
                request: RequestId(9),
                ..
            })
        ));
    }

    /// Asserts the `PaneOpened → Layout` order for the first pane, with
    /// the pane spawned at the solved window size.
    ///
    /// Case: the app starts and opens its first shell in an 80×24 window.
    #[test]
    fn the_root_pane_opens_at_the_window_size_and_reports_a_layout() {
        let mut h = Harness::new();
        h.resize(GridSize::new(80, 24).expect("a valid size"));
        h.drain();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(1),
            at: NewPaneAt::Root,
            cwd: None,
            env: vec![],
        });
        let mut events = h.drain();
        assert!(matches!(
            events.pop_front(),
            Some(OrzmuxEvent::PaneOpened {
                request: RequestId(1),
                ..
            })
        ));
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.panes.len(), 1);
        assert_eq!((layout.panes[0].cols, layout.panes[0].rows), (80, 24));
        assert_eq!(layout.active, Some(layout.panes[0].pane));
        assert!(frames.is_empty(), "a fresh pane has no resized sibling");
    }

    /// Asserts that a failed spawn rolls the tree back and reports
    /// `SpawnFailed` without a `Layout`.
    ///
    /// Case: the shell binary is missing when the user splits a pane.
    #[test]
    fn a_failed_split_spawn_rolls_back_and_reports_spawn_failed() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.log.fail_next.store(true, Ordering::Release);
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Id(root),
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.front(),
            Some(OrzmuxEvent::SpawnFailed {
                request: RequestId(2),
                ..
            })
        ));
        assert_eq!(h.backend.tree.panes(), vec![root]);
        assert_eq!(h.backend.tree.active(), Some(root));
    }

    /// Asserts that a split resizes the target pane and ships its
    /// repaint inside the `Layout` event.
    ///
    /// Case: the user splits the only pane vertically.
    #[test]
    fn a_split_resizes_the_target_and_bundles_its_frame_with_the_layout() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let mut events = h.drain();
        assert!(matches!(
            events.pop_front(),
            Some(OrzmuxEvent::PaneOpened {
                request: RequestId(2),
                ..
            })
        ));
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.panes.len(), 2);
        assert_eq!(frames.len(), 1, "only the shrunk root pane repaints");
        assert_eq!(frames[0].0, root);
        assert_eq!(frames[0].1.size, GridSize { cols: 40, rows: 24 });
        assert_eq!(h.backend.panes[&root].tty.pty_size().cols, 40);
        assert_eq!(
            h.log.sizes.lock().unwrap().last(),
            Some(&GridSize { cols: 39, rows: 24 })
        );
    }

    /// Asserts that a window resize re-solves every pane and bundles the
    /// changed panes' frames with the layout.
    ///
    /// Case: the user drags the window wider with two panes open.
    #[test]
    fn a_window_resize_reflows_every_pane() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        h.drain();
        h.resize(GridSize::new(120, 24).expect("a valid size"));
        let mut events = h.drain();
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_front() else {
            panic!("expected Layout");
        };
        assert_eq!(
            layout.size,
            GridSize {
                cols: 120,
                rows: 24
            }
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(h.backend.panes[&root].tty.pty_size().cols, 60);
        assert_eq!(h.backend.panes[&root].tty.pty_size().pixel_width, 60 * 8);
    }

    /// Asserts that a pane whose coalescer deadline passed is pumped by
    /// `service_deadlines` even though no chunk is ready.
    ///
    /// Case: another pane floods output so the select never times out,
    /// while this pane's 12 ms cap already elapsed.
    #[test]
    fn service_deadlines_pumps_a_pane_whose_deadline_passed() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.backend.pump_pane(root);
        h.drain();
        pane.chunk_tx.send(b"hello".to_vec()).unwrap();
        h.backend.pump_pane(root);
        h.drain();
        thread::sleep(Duration::from_millis(15));
        h.backend.service_deadlines();
        let events = h.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OrzmuxEvent::Frame { pane, .. } if *pane == root))
        );
    }

    /// Asserts that the depths recorded after a wake are the chunks
    /// still queued before the pump drains them.
    ///
    /// Case: a pane's reader queued two chunks while the backend slept
    /// and the `Select` just woke for that pane.
    #[test]
    fn record_queue_depths_sees_the_chunks_queued_before_the_pump() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx.send(b"a".to_vec()).unwrap();
        pane.chunk_tx.send(b"b".to_vec()).unwrap();
        h.backend.record_queue_depths();
        h.backend.pump_pane(root);
        let sample = h
            .backend
            .sampler
            .sample(Instant::now() + QueueSampler::SAMPLE_INTERVAL)
            .expect("a peak was recorded");
        assert_eq!(sample.chunks, vec![(root, ChunkDepth(2))]);
    }

    /// Asserts that the wake deadline is the sampler's report deadline
    /// while every pane is idle, and the earlier coalescer deadline
    /// once a pane has output pending.
    ///
    /// Case: a burst of output was recorded, then every pane went idle
    /// before the second elapsed, and the peak still has to be logged.
    #[test]
    fn the_wake_deadline_is_the_report_deadline_when_no_pane_deadline_is_earlier() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.backend.pump_pane(root);
        h.drain();
        assert_eq!(
            h.backend.next_wake_deadline(),
            None,
            "precondition: the pane is idle after its bootstrap frame and no peak is recorded"
        );
        h.backend.sampler.record_pane_depth(root, ChunkDepth(2));
        let report_deadline = h.backend.sampler.report_deadline();
        assert!(report_deadline.is_some());
        assert_eq!(h.backend.next_wake_deadline(), report_deadline);
        pane.chunk_tx.send(b"x".to_vec()).unwrap();
        h.backend.pump_pane(root);
        let pane_deadline = h.backend.panes[&root]
            .tty
            .next_deadline()
            .expect("pending output arms the coalescer");
        assert!(Some(pane_deadline) < report_deadline);
        assert_eq!(h.backend.next_wake_deadline(), Some(pane_deadline));
    }

    /// Splits the active pane and returns the new pane's id and its
    /// spawned fake terminal.
    fn split_active(h: &mut Harness, request: u64) -> (PaneId, FakePane) {
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(request),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        let Some(OrzmuxEvent::PaneOpened { pane, .. }) = events.front() else {
            panic!("expected PaneOpened, got {events:?}");
        };
        (*pane, h.panes.try_recv().expect("one spawned pane"))
    }

    /// Feeds `CSI ? 1004 h` through `pane`'s output stream and pumps it, so
    /// the pane's application has focus reporting enabled.
    fn enable_focus_reporting(h: &mut Harness, id: PaneId, pane: &FakePane) {
        pane.chunk_tx.send(b"\x1b[?1004h".to_vec()).unwrap();
        h.backend.pump_pane(id);
        h.drain();
        assert!(
            h.backend.panes[&id].tty.vt().modes().focus_in_out,
            "precondition: focus reporting is enabled"
        );
    }

    /// One wheel-up notch over cell (1, 1) with nothing held.
    fn wheel_up() -> WheelInput {
        WheelInput {
            up: 1,
            right: 0,
            mods: WheelModifiers::default(),
            cell: Some(CellCoord { col: 1, row: 1 }),
            report_mods: ProtocolModifiers::default(),
        }
    }

    /// Asserts that a `Wheel` command reaches its pane's terminal, which
    /// routes it by that pane's own modes.
    ///
    /// Case: nvim has turned on button-event tracking and SGR reports in
    /// a pane, and the user spins the wheel up over it.
    #[test]
    fn a_wheel_command_reaches_its_panes_terminal() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx
            .send(b"\x1b[?1002h\x1b[?1006h".to_vec())
            .unwrap();
        h.backend.pump_pane(root);
        h.drain();
        h.send(OrzmuxCommand::Wheel {
            pane: root,
            input: wheel_up(),
        });
        h.settle_writes();
        assert_eq!(pane.sink.contents(), b"\x1b[<64;1;1M");
    }

    /// Asserts that the wheel policy the backend was built with reaches
    /// each pane's terminal, so a configured `lines_per_notch` decides how
    /// many cursor keys one notch sends.
    ///
    /// Case: a user who set `lines_per_notch = 5` opens `less` in a pane
    /// and spins the wheel up one notch.
    #[test]
    fn the_backends_wheel_config_reaches_its_panes_terminal() {
        let mut h = Harness::with_wheel(WheelConfig {
            lines_per_notch: 5,
            ..WheelConfig::default()
        });
        let (root, pane) = h.open_root();
        pane.chunk_tx.send(b"\x1b[?1049h".to_vec()).unwrap();
        h.backend.pump_pane(root);
        h.drain();
        h.send(OrzmuxCommand::Wheel {
            pane: root,
            input: wheel_up(),
        });
        h.settle_writes();
        assert_eq!(pane.sink.contents(), b"\x1b[A".repeat(5));
    }

    /// Asserts that a `Wheel` for an unknown pane writes nothing and
    /// produces no event.
    ///
    /// Case: a wheel frame arrives for a pane the user closed a moment
    /// ago.
    #[test]
    fn a_wheel_command_for_an_unknown_pane_is_dropped_silently() {
        let mut h = Harness::new();
        let (_root, root_pane) = h.open_root();
        h.send(OrzmuxCommand::Wheel {
            pane: PaneId(99),
            input: wheel_up(),
        });
        assert!(h.drain().is_empty());
        assert!(root_pane.sink.contents().is_empty());
    }

    /// Asserts that `Active` targets resolve in command order, so a kill
    /// queued right after a split removes the new pane.
    ///
    /// Case: the user presses split then kill within one GUI frame,
    /// before any `Layout` has come back.
    #[test]
    fn active_targets_resolve_in_command_order() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        let (new, _new_pane) = split_active(&mut h, 2);
        h.send(OrzmuxCommand::KillPane {
            pane: PaneTarget::Active,
        });
        let events = h.drain();
        assert!(events.iter().any(|e| matches!(
            e,
            OrzmuxEvent::PaneClosed {
                pane,
                reason: CloseReason::Killed
            } if *pane == new
        )));
        assert_eq!(h.backend.tree.panes(), vec![root]);
        assert_eq!(h.backend.tree.active(), Some(root));
    }

    /// Asserts that a kill flushes the pane's pending output before
    /// `PaneClosed`, and that the survivor's resized frame rides in the
    /// following `Layout`.
    ///
    /// Case: the user kills a pane that had just printed something the
    /// coalescer had not yet emitted.
    #[test]
    fn kill_flushes_the_pane_then_closes_and_reflows() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        // NOTE: this pump settles the new pane's bootstrap frame so the
        // write below lands inside an ordinary debounce window instead of
        // being swept into the bootstrap snapshot, which the coalescer
        // always emits on a pane's very first pump regardless of damage.
        h.backend.pump_pane(new);
        h.drain();
        new_pane.chunk_tx.send(b"last words".to_vec()).unwrap();
        h.backend.pump_pane(new);
        h.drain();
        h.send(OrzmuxCommand::KillPane {
            pane: PaneTarget::Id(new),
        });
        let events: Vec<OrzmuxEvent> = h.drain().into_iter().collect();
        let closed_at = events
            .iter()
            .position(|e| matches!(e, OrzmuxEvent::PaneClosed { pane, .. } if *pane == new))
            .expect("PaneClosed");
        let frame_at = events
            .iter()
            .position(|e| matches!(e, OrzmuxEvent::Frame { pane, .. } if *pane == new))
            .expect("a final Frame for the killed pane");
        assert!(frame_at < closed_at, "the final frame precedes PaneClosed");
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.last() else {
            panic!("Layout must be last");
        };
        assert_eq!(layout.panes.len(), 1);
        assert_eq!(
            frames.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            vec![root]
        );
    }

    /// Asserts that a child exit closes its pane after the remaining
    /// output, and that closing the last pane yields an empty layout.
    ///
    /// Case: the user types `exit` in the only pane.
    #[test]
    fn a_child_exit_closes_the_pane_and_the_last_one_empties_the_layout() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx.send(b"logout\r\n".to_vec()).unwrap();
        pane.exit_tx.send(Some(0)).unwrap();
        drop(pane.chunk_tx);
        drop(pane.exit_tx);
        h.backend.pump_pane(root);
        let events: Vec<OrzmuxEvent> = h.drain().into_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e,
            OrzmuxEvent::PaneClosed {
                pane,
                reason: CloseReason::ChildExit { code: Some(0) }
            } if *pane == root
        )));
        let Some(OrzmuxEvent::Layout { layout, .. }) = events.last() else {
            panic!("Layout must be last");
        };
        assert!(layout.panes.is_empty());
        assert!(h.backend.tree.is_empty());
    }

    /// Asserts that keyboard input reaches the active pane's PTY and that
    /// a `SelectPane` always answers with a `Layout`, even when refused.
    ///
    /// Case: the user clicks a pane that closed a moment ago, then types.
    #[test]
    fn select_pane_always_answers_with_a_layout_and_keys_reach_the_active_pane() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let seq = h.send(OrzmuxCommand::SelectPane { pane: PaneId(99) });
        let events = h.drain();
        let Some(OrzmuxEvent::Layout { layout, .. }) = events.front() else {
            panic!("a refused SelectPane still answers with a Layout");
        };
        assert_eq!(layout.seq, seq);
        assert_eq!(layout.active, Some(root));

        h.send(OrzmuxCommand::KeyInput {
            pane: PaneTarget::Active,
            key: TerminalKey::Character(KeyText::new("a").unwrap()),
            mods: TerminalModifiers::default(),
        });
        h.settle_writes();
        assert_eq!(root_pane.sink.contents(), b"a");
    }

    /// Asserts that a `KeyInput` for an unknown pane writes nothing and
    /// produces no event, rather than panicking or falling back to
    /// another pane.
    ///
    /// Case: a stale keystroke arrives for a pane the user already closed.
    #[test]
    fn key_input_for_an_unknown_pane_is_dropped_silently() {
        let mut h = Harness::new();
        let (_root, root_pane) = h.open_root();
        h.send(OrzmuxCommand::KeyInput {
            pane: PaneTarget::Id(PaneId(99)),
            key: TerminalKey::Character(KeyText::new("a").unwrap()),
            mods: TerminalModifiers::default(),
        });
        assert!(h.drain().is_empty());
        h.settle_writes();
        assert!(root_pane.sink.contents().is_empty());
    }

    /// Asserts that directional selection moves the active pane and
    /// answers with a `Layout` carrying the new active.
    ///
    /// Case: the user presses select-left-pane from the right pane.
    #[test]
    fn select_direction_changes_the_active_pane() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        let (_new, _new_pane) = split_active(&mut h, 2);
        h.send(OrzmuxCommand::SelectPaneDirection {
            direction: PaneDirection::Left,
        });
        let events = h.drain();
        let Some(OrzmuxEvent::Layout { layout, .. }) = events.front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.active, Some(root));
    }

    /// Asserts that a directional selection with no neighbour in that
    /// direction publishes nothing.
    ///
    /// Case: the user holds select-left with the leftmost pane already
    /// active.
    #[test]
    fn select_direction_into_a_wall_publishes_nothing() {
        let mut h = Harness::new();
        let (_root, _root_pane) = h.open_root();
        h.drain();
        h.send(OrzmuxCommand::SelectPaneDirection {
            direction: PaneDirection::Left,
        });
        assert!(h.drain().is_empty());
    }

    /// Asserts that every `CopySelection` is answered exactly once, with
    /// `None`s when the target cannot be resolved.
    ///
    /// Case: the user presses copy with no selection, and a stale copy
    /// aimed at a pane that no longer exists arrives.
    #[test]
    fn copy_selection_is_always_answered() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        h.send(OrzmuxCommand::CopySelection {
            pane: PaneTarget::Id(root),
        });
        h.send(OrzmuxCommand::CopySelection {
            pane: PaneTarget::Id(PaneId(42)),
        });
        let answers = h
            .drain()
            .into_iter()
            .filter(|event| *event == OrzmuxEvent::SelectionText { text: None })
            .count();
        assert_eq!(answers, 2);
    }

    /// Asserts that a split inherits the target pane's last OSC 7
    /// directory when the GUI passes `cwd: None` and the OS reports no
    /// directory for the target's process.
    ///
    /// Case: a shell that reports its directory through OSC 7 `cd`s into a
    /// project while the OS cannot be asked for the pane's directory, and
    /// the user splits the pane.
    #[test]
    fn a_split_inherits_the_target_panes_reported_cwd() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx
            .send(b"\x1b]7;file://localhost/tmp/project\x1b\\".to_vec())
            .unwrap();
        h.backend.pump_pane(root);
        h.drain();
        assert_eq!(
            h.backend.panes[&root].cwd().as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        h.log.cwds.lock().unwrap().clear();
        split_active(&mut h, 2);
        assert_eq!(
            h.log.cwds.lock().unwrap().last().and_then(|c| c.as_deref()),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    /// Asserts that a split given an explicit directory spawns there rather
    /// than in the target pane's directory.
    ///
    /// Case: a caller opens a split in a directory it names itself while
    /// the target pane's shell has reported another.
    #[test]
    fn an_explicit_cwd_wins_over_the_target_panes_cwd() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx
            .send(b"\x1b]7;file://localhost/tmp/project\x1b\\".to_vec())
            .unwrap();
        h.backend.pump_pane(root);
        h.drain();
        h.log.cwds.lock().unwrap().clear();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: Some(PathBuf::from("/explicit")),
            env: vec![],
        });
        h.drain();
        assert_eq!(
            h.log.cwds.lock().unwrap().last().cloned().flatten(),
            Some(PathBuf::from("/explicit"))
        );
    }

    /// Asserts that a pane spawned in an inherited directory passes that
    /// directory on when it is split before its own shell has reported
    /// one and while the OS reports no directory for its process.
    ///
    /// Case: the user splits twice in quick succession while the new shell
    /// has not printed its first prompt and the OS cannot be asked for the
    /// new pane's directory.
    #[test]
    fn a_split_from_a_pane_that_has_not_reported_a_cwd_passes_on_its_spawn_cwd() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx
            .send(b"\x1b]7;file://localhost/tmp/project\x1b\\".to_vec())
            .unwrap();
        h.backend.pump_pane(root);
        h.drain();
        split_active(&mut h, 2);
        h.log.cwds.lock().unwrap().clear();
        split_active(&mut h, 3);
        assert_eq!(
            h.log.cwds.lock().unwrap().last().and_then(|c| c.as_deref()),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    /// Asserts that selecting a pane reports focus loss to the pane it
    /// leaves and focus gain to the pane it enters.
    ///
    /// Case: the user moves from one nvim pane to another and back.
    #[test]
    fn select_pane_moves_the_focus_report_from_the_old_pane_to_the_new() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        enable_focus_reporting(&mut h, new, &new_pane);
        h.send(OrzmuxCommand::SelectPane { pane: root });
        h.send(OrzmuxCommand::SelectPane { pane: new });
        h.settle_writes();
        assert_eq!(root_pane.sink.contents(), b"\x1b[I\x1b[O");
        assert_eq!(new_pane.sink.contents(), b"\x1b[O\x1b[I");
    }

    /// Asserts that a split reports focus loss to the pane it splits and
    /// nothing to the pane it creates, even when the new pane's unread
    /// start-up output enables focus reporting.
    ///
    /// Case: the user splits the pane running nvim on Windows, where a new
    /// pane's start-up output enables focus reporting.
    #[test]
    fn a_split_reports_focus_loss_to_the_split_pane_only() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        enable_focus_reporting(&mut h, root, &root_pane);
        *h.log.spawn_output.lock().unwrap() = Some(b"\x1b[?1004h".to_vec());
        let (_new, new_pane) = split_active(&mut h, 2);
        h.settle_writes();
        assert_eq!(root_pane.sink.contents(), b"\x1b[O");
        assert_eq!(new_pane.sink.contents(), b"");
    }

    /// Asserts that a split whose spawn fails reports no focus change.
    ///
    /// Case: the shell binary is missing when the user splits the pane
    /// running nvim.
    #[test]
    fn a_failed_split_reports_no_focus_change() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        enable_focus_reporting(&mut h, root, &root_pane);
        h.log.fail_next.store(true, Ordering::Release);
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Id(root),
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        assert!(matches!(
            h.drain().front(),
            Some(OrzmuxEvent::SpawnFailed { .. })
        ));
        h.settle_writes();
        assert_eq!(root_pane.sink.contents(), b"");
    }

    /// Asserts that killing the active pane reports focus gain to the pane
    /// that becomes active.
    ///
    /// Case: the user closes a scratch pane and lands back in nvim.
    #[test]
    fn killing_the_active_pane_reports_focus_to_the_survivor() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let (_new, _new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        h.send(OrzmuxCommand::KillPane {
            pane: PaneTarget::Active,
        });
        h.settle_writes();
        assert_eq!(root_pane.sink.contents(), b"\x1b[I");
    }

    /// Asserts that window focus changes reach only the active pane, and
    /// only when the focus state changes.
    ///
    /// Case: the user switches from orzma to a browser, the loss is
    /// reported twice, and the user switches back.
    #[test]
    fn window_focus_reports_to_the_active_pane_only_on_change() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        enable_focus_reporting(&mut h, new, &new_pane);
        h.send(OrzmuxCommand::WindowFocus { focused: false });
        h.send(OrzmuxCommand::WindowFocus { focused: false });
        h.send(OrzmuxCommand::WindowFocus { focused: true });
        h.settle_writes();
        assert_eq!(new_pane.sink.contents(), b"\x1b[O\x1b[I");
        assert_eq!(root_pane.sink.contents(), b"");
    }

    /// Asserts that a failed focus write to the pane being left does not
    /// stop the report to the pane being entered.
    ///
    /// Case: the user moves from a pane whose PTY rejects writes into a
    /// pane running nvim.
    #[test]
    fn a_failed_focus_write_does_not_stop_the_incoming_report() {
        let mut h = Harness::new();
        h.log.fail_writes_next.store(true, Ordering::Release);
        let (root, root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        enable_focus_reporting(&mut h, new, &new_pane);
        h.send(OrzmuxCommand::SelectPane { pane: root });
        h.settle_writes();
        h.send(OrzmuxCommand::SelectPane { pane: new });
        h.settle_writes();
        assert_eq!(new_pane.sink.contents(), b"\x1b[O\x1b[I");
    }

    /// Asserts that a pane whose PTY stops accepting input neither stalls
    /// commands for other panes nor survives `KillPane`.
    ///
    /// Case: the user stops nvim under a debugger in one pane, keeps typing
    /// and pastes into it, then types in the other pane and kills the stuck
    /// one.
    #[test]
    fn a_pane_that_stops_reading_does_not_freeze_the_backend() {
        let gate = BlockingSink::default();
        let thread_gate = gate.clone();
        let (done_tx, done_rx) = bounded::<(Vec<u8>, bool)>(1);
        thread::spawn(move || {
            let mut h = Harness::new();
            let stuck_writer = thread_gate.clone();
            *h.log.block_writes_next.lock().unwrap() = Some(thread_gate);
            let (stuck, _stuck_pane) = h.open_root();
            let (other, other_pane) = split_active(&mut h, 2);
            for text in ["x", "y"] {
                h.send(OrzmuxCommand::KeyInput {
                    pane: PaneTarget::Id(stuck),
                    key: TerminalKey::Character(KeyText::new(text).unwrap()),
                    mods: TerminalModifiers::default(),
                });
            }
            h.send(OrzmuxCommand::Paste {
                pane: PaneTarget::Id(stuck),
                text: "a pasted line".to_string(),
            });
            stuck_writer.wait_until_writing();
            h.send(OrzmuxCommand::KeyInput {
                pane: PaneTarget::Id(other),
                key: TerminalKey::Character(KeyText::new("z").unwrap()),
                mods: TerminalModifiers::default(),
            });
            h.backend.panes[&other].tty.settle_writes();
            let other_received = other_pane.sink.contents();
            h.send(OrzmuxCommand::KillPane {
                pane: PaneTarget::Id(stuck),
            });
            let _ = done_tx.send((other_received, !h.backend.panes.contains_key(&stuck)));
        });
        let outcome = done_rx.recv_timeout(Duration::from_secs(10));
        gate.release();
        let (other_received, stuck_closed) = match outcome {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                panic!("the backend froze on a pane that stopped reading stdin")
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("the harness thread panicked before reporting")
            }
        };
        assert_eq!(other_received, b"z");
        assert!(stuck_closed);
    }

    /// Asserts that a resize publishes one layout whose divider moved
    /// and repaints only the panes whose size changed.
    ///
    /// Case: the user drags the divider of a two-pane window to the
    /// right, widening the left pane.
    #[test]
    fn a_resize_publishes_a_moved_layout_and_repaints_the_resized_panes() {
        let mut h = Harness::new();
        let (_root, _pane) = h.open_root();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let mut opened = h.drain();
        let Some(OrzmuxEvent::Layout { layout, .. }) = opened.pop_back() else {
            panic!("expected a Layout after the split");
        };
        let split = layout.separators[0].split;

        h.send(OrzmuxCommand::ResizeSplit {
            split,
            position: 60,
        });
        let mut events = h.drain();

        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_back() else {
            panic!("expected a Layout event");
        };
        assert_eq!(layout.separators[0].x, 60);
        assert_eq!(frames.len(), 2);
    }

    /// Asserts that a resize naming a split the tree does not have
    /// publishes nothing at all.
    ///
    /// Case: the pane the pointer was resizing closed a frame earlier,
    /// so the drag's next command names a divider that is gone.
    #[test]
    fn a_resize_of_an_unknown_split_publishes_nothing() {
        let mut h = Harness::new();
        let (_root, _pane) = h.open_root();
        h.drain();

        h.send(OrzmuxCommand::ResizeSplit {
            split: SplitId(999),
            position: 60,
        });

        assert!(h.drain().is_empty());
    }
}
