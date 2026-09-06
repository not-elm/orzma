//! The multiplexer backend loop: owns every pane, waits on the command
//! channel and each pane's PTY streams with one `Select`, and emits
//! layout / frame / signal events to the GUI.

use crate::backend::pane::{Pane, PaneFactory};
use crate::layout::LayoutTree;
use crate::protocol::{
    CloseReason, CommandSeq, Layout, MuxCommand, MuxEvent, NewPaneAt, PaneId, PaneTarget, RequestId,
};
use crossbeam_channel::{Receiver, Select, Sender, TryRecvError};
use orzma_tty::CellPixels;
use orzma_tty::prelude::{PumpOutput, TtySignal};
use orzma_vt::prelude::{Frame, GridSize, Vt, VtSignal};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

pub(crate) mod pane;
pub(crate) use pane::ShellFactory;

/// The backend state, driven by [`Backend::run`] on its own thread.
pub(crate) struct Backend {
    factory: Box<dyn PaneFactory>,
    panes: HashMap<PaneId, Pane>,
    tree: LayoutTree,
    geometry: Option<Geometry>,
    commands: Receiver<(CommandSeq, MuxCommand)>,
    events: Sender<MuxEvent>,
    next_pane_id: u32,
    processed: CommandSeq,
    /// Set when the GUI's event receiver is gone; the loop exits.
    gui_gone: bool,
}

impl Backend {
    /// A backend with no panes and no geometry.
    pub(crate) fn new(
        factory: Box<dyn PaneFactory>,
        commands: Receiver<(CommandSeq, MuxCommand)>,
        events: Sender<MuxEvent>,
    ) -> Self {
        Self {
            factory,
            panes: HashMap::new(),
            tree: LayoutTree::new(),
            geometry: None,
            commands,
            events,
            next_pane_id: 1,
            processed: CommandSeq::default(),
            gui_gone: false,
        }
    }

    /// Runs until the command channel disconnects (the GUI dropped its
    /// client) or the GUI stops receiving events. Dropping the panes on
    /// return kills every child.
    pub(crate) fn run(mut self) {
        loop {
            match self.wait_ready() {
                Some(Ready::Commands) if !self.drain_commands() => return,
                Some(Ready::Commands) => {}
                Some(Ready::Pane(pane)) => self.pump_pane(pane),
                None => {}
            }
            self.service_deadlines();
            if self.gui_gone {
                return;
            }
        }
    }

    /// Applies one command. `pub(crate)` so tests drive the backend
    /// without a thread.
    pub(crate) fn handle_command(&mut self, seq: CommandSeq, command: MuxCommand) {
        self.processed = seq;
        match command {
            MuxCommand::Resize {
                cols,
                rows,
                cell_px,
            } => self.on_resize(cols, rows, cell_px),
            MuxCommand::NewPane {
                request,
                at,
                cwd,
                env,
            } => self.on_new_pane(request, at, cwd, env),
            other => self.handle_pane_command(other),
        }
    }

    /// Pumps one pane and forwards its output; closes the pane on
    /// `ChildExit`.
    pub(crate) fn pump_pane(&mut self, id: PaneId) {
        let Some(pane) = self.panes.get_mut(&id) else {
            return;
        };
        let output = pane.tty.pump();
        let exited = self.emit_pump_output(id, output);
        if let Some(code) = exited {
            self.close_pane(id, CloseReason::ChildExit { code });
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
    /// coalescer deadline passes. Returns the ready source, `None` on
    /// timeout. The `Select` is dropped before returning so the pane
    /// receivers it borrowed can be pumped.
    fn wait_ready(&self) -> Option<Ready> {
        let mut select = Select::new();
        let mut sources: Vec<Ready> = Vec::with_capacity(1 + self.panes.len() * 2);
        select.recv(&self.commands);
        sources.push(Ready::Commands);
        for (id, pane) in &self.panes {
            let readiness = pane.tty.readiness();
            select.recv(readiness.chunks);
            sources.push(Ready::Pane(*id));
            if let Some(exit) = readiness.exit {
                select.recv(exit);
                sources.push(Ready::Pane(*id));
            }
        }
        let deadline = self
            .panes
            .values()
            .filter_map(|p| p.tty.next_deadline())
            .min();
        let index = match deadline {
            Some(deadline) => select.ready_deadline(deadline).ok()?,
            None => select.ready(),
        };
        Some(sources[index])
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

    fn on_resize(&mut self, cols: u16, rows: u16, cell_px: CellPixels) {
        self.geometry = Some(Geometry {
            size: GridSize { cols, rows },
            cell_px,
        });
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
            self.emit(MuxEvent::SpawnFailed {
                request,
                error: "no geometry".to_string(),
            });
            return;
        };
        let new = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let previous_active = self.tree.active();
        let inherited_cwd = match at {
            NewPaneAt::Root => {
                if self.tree.insert_root(new).is_err() {
                    self.emit(MuxEvent::SpawnFailed {
                        request,
                        error: "root already open".to_string(),
                    });
                    return;
                }
                None
            }
            NewPaneAt::Split { pane, orientation } => {
                let Some(target) = self.resolve(pane) else {
                    self.emit(MuxEvent::SpawnFailed {
                        request,
                        error: "no target pane".to_string(),
                    });
                    return;
                };
                if self
                    .tree
                    .split(target, orientation, new, geometry.size)
                    .is_err()
                {
                    self.emit(MuxEvent::SpawnFailed {
                        request,
                        error: "no space".to_string(),
                    });
                    return;
                }
                self.panes.get(&target).and_then(|p| p.cwd.clone())
            }
        };
        let solved = self.tree.solve(geometry.size);
        let rect = solved
            .panes
            .iter()
            .find(|r| r.pane == new)
            .expect("the new pane is in the tree");
        let size = GridSize {
            cols: rect.cols,
            rows: rect.rows,
        };
        let spawn_cwd = cwd.or(inherited_cwd);
        match self
            .factory
            .spawn(size, geometry.cell_px, spawn_cwd.clone(), env)
        {
            Ok(tty) => {
                self.panes.insert(
                    new,
                    Pane {
                        tty,
                        applied: (size.cols, size.rows, geometry.cell_px),
                        cwd: spawn_cwd,
                    },
                );
                self.emit(MuxEvent::PaneOpened { pane: new, request });
                self.publish_layout();
            }
            Err(err) => {
                self.tree.remove(new);
                if let Some(previous) = previous_active {
                    self.tree.select(previous);
                }
                self.emit(MuxEvent::SpawnFailed {
                    request,
                    error: err.to_string(),
                });
            }
        }
    }

    /// Re-solves the tree, resizes every pane whose applied geometry
    /// differs, flushes those panes, and emits their signals followed by
    /// one `Layout` carrying their frames. A no-op without geometry.
    fn publish_layout(&mut self) {
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
            match pane.tty.resize(rect.cols, rect.rows, geometry.cell_px) {
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
            self.emit(MuxEvent::Signal { pane, signal });
        }
        let layout = Layout {
            seq: self.processed,
            size: geometry.size,
            active: self.tree.active(),
            panes: solved.panes,
            separators: solved.separators,
        };
        self.emit(MuxEvent::Layout { layout, frames });
    }

    /// Forwards a pump's frame and signals. Returns `Some(code)` when the
    /// signals carried `ChildExit`.
    fn emit_pump_output(&mut self, id: PaneId, output: PumpOutput) -> Option<Option<i32>> {
        let mut exited = None;
        for signal in output.signals {
            match signal {
                TtySignal::ChildExit { code } => exited = Some(code),
                TtySignal::Vt(signal) => {
                    if let VtSignal::CurrentDir(path) = &signal
                        && let Some(pane) = self.panes.get_mut(&id)
                    {
                        pane.cwd = Some(path.clone());
                    }
                    self.emit(MuxEvent::Signal { pane: id, signal });
                }
            }
        }
        if let Some(frame) = output.frame {
            self.emit(MuxEvent::Frame { pane: id, frame });
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
        self.emit(MuxEvent::PaneClosed { pane: id, reason });
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

    /// Resolves a target to its pane's mutable state, logging via
    /// [`Self::resolve_or_log`] when it does not resolve.
    fn pane_mut(&mut self, target: PaneTarget, command: &'static str) -> Option<&mut Pane> {
        let id = self.resolve_or_log(target, command)?;
        self.panes.get_mut(&id)
    }

    fn emit(&mut self, event: MuxEvent) {
        if self.events.send(event).is_err() {
            self.gui_gone = true;
        }
    }

    /// Applies a pane-level command. Unknown panes and an unresolvable
    /// `Active` are dropped with a debug log; `CopySelection` always
    /// answers and `SelectPane*` always publishes a layout.
    fn handle_pane_command(&mut self, command: MuxCommand) {
        match command {
            MuxCommand::KillPane { pane } => {
                if let Some(id) = self.resolve_or_log(pane, "KillPane") {
                    self.close_pane(id, CloseReason::Killed);
                }
            }
            MuxCommand::SelectPane { pane } => {
                if !self.tree.select(pane) {
                    tracing::debug!(?pane, "select of an unknown pane refused");
                }
                self.publish_layout();
            }
            MuxCommand::SelectPaneDirection { direction } => {
                if let Some(geometry) = self.geometry {
                    self.tree.select_direction(direction, geometry.size);
                }
                self.publish_layout();
            }
            MuxCommand::KeyInput { pane, key, mods } => {
                if let Some(p) = self.pane_mut(pane, "KeyInput")
                    && let Err(err) = p.tty.send_key(&key, &mods)
                {
                    tracing::error!(%err, "key write failed");
                }
            }
            MuxCommand::Paste { pane, text } => {
                if let Some(p) = self.pane_mut(pane, "Paste")
                    && let Err(err) = p.tty.send_paste(&text)
                {
                    tracing::error!(%err, "paste write failed");
                }
            }
            MuxCommand::MouseInput { pane, report } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "MouseInput")
                    && let Err(err) = p.tty.send_mouse(report)
                {
                    tracing::error!(%err, "mouse write failed");
                }
            }
            MuxCommand::Scroll { pane, scroll } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "Scroll") {
                    p.tty.scroll(scroll);
                }
            }
            MuxCommand::SelectionStart {
                pane,
                cell,
                side,
                kind,
            } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "SelectionStart") {
                    p.tty.start_selection(cell, side, kind);
                }
            }
            MuxCommand::SelectionUpdate { pane, cell, side } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "SelectionUpdate") {
                    p.tty.extend_selection(cell, side);
                }
            }
            MuxCommand::SelectionClear { pane } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "SelectionClear") {
                    p.tty.clear_selection();
                }
            }
            MuxCommand::CopySelection { pane, request } => {
                let resolved = self.resolve(pane);
                let text = resolved
                    .and_then(|id| self.panes.get(&id))
                    .and_then(|p| p.tty.vt().selection_text())
                    .filter(|t| !t.is_empty());
                self.emit(MuxEvent::SelectionText {
                    request,
                    pane: resolved,
                    text,
                });
            }
            MuxCommand::RemovePlacements { pane, instances } => {
                if let Some(p) = self.pane_mut(PaneTarget::Id(pane), "RemovePlacements") {
                    p.tty.remove_placements(&instances);
                }
            }
            MuxCommand::Resize { .. } | MuxCommand::NewPane { .. } => {
                unreachable!("handled by handle_command")
            }
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

/// How many queued commands one iteration applies before pumping panes.
const COMMAND_BATCH: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::{PaneDirection, SplitOrientation};
    use crossbeam_channel::{Receiver, Sender, unbounded};
    use orzma_tty::prelude::{
        KeyText, OrzmaTty, OrzmaTtyError, OrzmaTtyResult, TerminalKey, TerminalModifiers,
    };
    use orzma_tty::test_support::CaptureSink;
    use orzma_vt::prelude::OrzmaVt;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// The test's ends of one spawned pane's streams.
    struct FakePane {
        chunk_tx: Sender<Vec<u8>>,
        exit_tx: Sender<Option<i32>>,
        sink: CaptureSink,
    }

    /// What the factory recorded, shared with the harness through `Arc`
    /// because the backend only holds the factory as `dyn PaneFactory`.
    #[derive(Default)]
    struct FactoryLog {
        fail_next: AtomicBool,
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
            let sink = CaptureSink::default();
            let tty = OrzmaTty::detached_with_channels(
                OrzmaVt::new(size, 100),
                size.cols,
                size.rows,
                Box::new(sink.clone()),
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
        events: Receiver<MuxEvent>,
        panes: Receiver<FakePane>,
        log: Arc<FactoryLog>,
        seq: u64,
    }

    impl Harness {
        fn new() -> Self {
            let (spawned_tx, spawned_rx) = unbounded();
            let (_command_tx, command_rx) = unbounded();
            let (event_tx, event_rx) = unbounded();
            let log = Arc::new(FactoryLog::default());
            let factory = FakeFactory {
                spawned: spawned_tx,
                log: Arc::clone(&log),
            };
            Self {
                backend: Backend::new(Box::new(factory), command_rx, event_tx),
                events: event_rx,
                panes: spawned_rx,
                log,
                seq: 0,
            }
        }

        fn send(&mut self, command: MuxCommand) -> CommandSeq {
            self.seq += 1;
            let seq = CommandSeq(self.seq);
            self.backend.handle_command(seq, command);
            seq
        }

        fn drain(&self) -> VecDeque<MuxEvent> {
            self.events.try_iter().collect()
        }

        fn resize(&mut self, cols: u16, rows: u16) {
            self.send(MuxCommand::Resize {
                cols,
                rows,
                cell_px: CellPixels {
                    width: 8,
                    height: 16,
                },
            });
        }

        fn open_root(&mut self) -> (PaneId, FakePane) {
            self.resize(80, 24);
            self.drain();
            self.send(MuxCommand::NewPane {
                request: RequestId(1),
                at: NewPaneAt::Root,
                cwd: None,
                env: vec![],
            });
            let events = self.drain();
            let Some(MuxEvent::PaneOpened { pane, .. }) = events.front() else {
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
        h.send(MuxCommand::NewPane {
            request: RequestId(9),
            at: NewPaneAt::Root,
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        assert!(matches!(
            events.front(),
            Some(MuxEvent::SpawnFailed {
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
        h.resize(80, 24);
        h.drain();
        h.send(MuxCommand::NewPane {
            request: RequestId(1),
            at: NewPaneAt::Root,
            cwd: None,
            env: vec![],
        });
        let mut events = h.drain();
        assert!(matches!(
            events.pop_front(),
            Some(MuxEvent::PaneOpened {
                request: RequestId(1),
                ..
            })
        ));
        let Some(MuxEvent::Layout { layout, frames }) = events.pop_front() else {
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
        h.send(MuxCommand::NewPane {
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
            Some(MuxEvent::SpawnFailed {
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
        h.send(MuxCommand::NewPane {
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
            Some(MuxEvent::PaneOpened {
                request: RequestId(2),
                ..
            })
        ));
        let Some(MuxEvent::Layout { layout, frames }) = events.pop_front() else {
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
        h.send(MuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        h.drain();
        h.resize(120, 24);
        let mut events = h.drain();
        let Some(MuxEvent::Layout { layout, frames }) = events.pop_front() else {
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
        std::thread::sleep(std::time::Duration::from_millis(15));
        h.backend.service_deadlines();
        let events = h.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MuxEvent::Frame { pane, .. } if *pane == root))
        );
    }

    /// Splits the active pane and returns the new pane's id and its
    /// spawned fake terminal.
    fn split_active(h: &mut Harness, request: u64) -> (PaneId, FakePane) {
        h.send(MuxCommand::NewPane {
            request: RequestId(request),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        let Some(MuxEvent::PaneOpened { pane, .. }) = events.front() else {
            panic!("expected PaneOpened, got {events:?}");
        };
        (*pane, h.panes.try_recv().expect("one spawned pane"))
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
        h.send(MuxCommand::KillPane {
            pane: PaneTarget::Active,
        });
        let events = h.drain();
        assert!(events.iter().any(|e| matches!(
            e,
            MuxEvent::PaneClosed {
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
        h.send(MuxCommand::KillPane {
            pane: PaneTarget::Id(new),
        });
        let events: Vec<MuxEvent> = h.drain().into_iter().collect();
        let closed_at = events
            .iter()
            .position(|e| matches!(e, MuxEvent::PaneClosed { pane, .. } if *pane == new))
            .expect("PaneClosed");
        let frame_at = events
            .iter()
            .position(|e| matches!(e, MuxEvent::Frame { pane, .. } if *pane == new))
            .expect("a final Frame for the killed pane");
        assert!(frame_at < closed_at, "the final frame precedes PaneClosed");
        let Some(MuxEvent::Layout { layout, frames }) = events.last() else {
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
        let events: Vec<MuxEvent> = h.drain().into_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e,
            MuxEvent::PaneClosed {
                pane,
                reason: CloseReason::ChildExit { code: Some(0) }
            } if *pane == root
        )));
        let Some(MuxEvent::Layout { layout, .. }) = events.last() else {
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
        let seq = h.send(MuxCommand::SelectPane { pane: PaneId(99) });
        let events = h.drain();
        let Some(MuxEvent::Layout { layout, .. }) = events.front() else {
            panic!("a refused SelectPane still answers with a Layout");
        };
        assert_eq!(layout.seq, seq);
        assert_eq!(layout.active, Some(root));

        h.send(MuxCommand::KeyInput {
            pane: PaneTarget::Active,
            key: TerminalKey::Character(KeyText::new("a").unwrap()),
            mods: TerminalModifiers::default(),
        });
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
        h.send(MuxCommand::KeyInput {
            pane: PaneTarget::Id(PaneId(99)),
            key: TerminalKey::Character(KeyText::new("a").unwrap()),
            mods: TerminalModifiers::default(),
        });
        assert!(h.drain().is_empty());
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
        h.send(MuxCommand::SelectPaneDirection {
            direction: PaneDirection::Left,
        });
        let events = h.drain();
        let Some(MuxEvent::Layout { layout, .. }) = events.front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.active, Some(root));
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
        h.send(MuxCommand::CopySelection {
            pane: PaneTarget::Id(root),
            request: RequestId(5),
        });
        h.send(MuxCommand::CopySelection {
            pane: PaneTarget::Id(PaneId(42)),
            request: RequestId(6),
        });
        let events: Vec<MuxEvent> = h.drain().into_iter().collect();
        assert!(events.contains(&MuxEvent::SelectionText {
            request: RequestId(5),
            pane: Some(root),
            text: None,
        }));
        assert!(events.contains(&MuxEvent::SelectionText {
            request: RequestId(6),
            pane: None,
            text: None,
        }));
    }

    /// Asserts that a split inherits the target pane's last OSC 7
    /// directory when the GUI passes `cwd: None`.
    ///
    /// Case: the shell `cd`s into a project and the user splits the pane.
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
            h.backend.panes[&root].cwd.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        h.log.cwds.lock().unwrap().clear();
        split_active(&mut h, 2);
        assert_eq!(
            h.log.cwds.lock().unwrap().last().and_then(|c| c.as_deref()),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    /// Asserts that a pane spawned in an inherited directory passes that
    /// directory on when it is split before its own shell has reported
    /// one.
    ///
    /// Case: the user splits twice in quick succession while the new
    /// shell is still starting up and has not printed its first prompt.
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
}
