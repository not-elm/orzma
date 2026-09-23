//! Test fixtures for the multiplexer: PTY-less panes, a factory that
//! hands the test their input ends, and a harness that drives one
//! event loop.

use crate::backend::pane::PaneFactory;
use crate::backend::{Backend, CommandSeq, NewPaneAt, OrzmuxEvent, PaneId, RequestId};
use crate::error::OrzmuxResult;
use crate::event_loop::{EventLoop, GuiLink, OrzmuxCommand};
use crossbeam_channel::{Receiver, Sender, unbounded};
use orzma_tty::prelude::{OrzmaTty, OrzmaTtyError, WheelConfig};
use orzma_tty::test_support::{BlockingSink, CaptureSink, FailingSink};
use orzma_tty::{CellPixels, EnvKey, EnvValue};
use orzma_vt::prelude::{GridSize, OrzmaVt};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};

/// The test's ends of one spawned pane's streams.
pub(crate) struct FakePane {
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
    sink: CaptureSink,
}

impl FakePane {
    /// Feeds `bytes` to the pane's output stream, as if its application
    /// printed them.
    pub fn print(&self, bytes: &[u8]) {
        self.chunk_tx
            .send(bytes.to_vec())
            .expect("the pane's terminal holds the output receiver");
    }

    /// Ends the pane's application with the exit status `code`.
    pub fn exit(&self, code: Option<i32>) {
        self.exit_tx
            .send(code)
            .expect("the pane's terminal holds the exit receiver");
    }

    /// The bytes the backend has written to the pane's PTY so far.
    pub fn received(&self) -> Vec<u8> {
        self.sink.contents()
    }
}

/// What the factory recorded, shared with the harness through `Arc`.
#[derive(Default)]
pub(crate) struct FactoryLog {
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
pub(crate) struct FakeFactory {
    spawned: Sender<FakePane>,
    log: Arc<FactoryLog>,
}

impl FakeFactory {
    /// A factory that hands each spawned pane's test ends to `spawned` and
    /// records every spawn request in `log`.
    pub fn new(spawned: Sender<FakePane>, log: Arc<FactoryLog>) -> Self {
        Self { spawned, log }
    }
}

impl PaneFactory for FakeFactory {
    fn spawn(
        &mut self,
        size: GridSize,
        _cell_px: CellPixels,
        cwd: Option<PathBuf>,
        _env: Vec<(EnvKey, EnvValue)>,
    ) -> OrzmuxResult<OrzmaTty<OrzmaVt>> {
        self.log.sizes.lock().unwrap().push(size);
        self.log.cwds.lock().unwrap().push(cwd);
        if self.log.fail_next.swap(false, Ordering::AcqRel) {
            return Err(OrzmaTtyError::SpawnShell(anyhow::anyhow!("injected")).into());
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

/// Counts the wakes the event loop sends the GUI.
#[derive(Default)]
pub(crate) struct WakeCount(AtomicUsize);

impl WakeCount {
    /// The number of wakes sent so far.
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

/// Drives one [`EventLoop`] whose panes are spawned by a PTY-less
/// factory.
pub(crate) struct Harness {
    event_loop: EventLoop,
    events: Receiver<OrzmuxEvent>,
    panes: Receiver<FakePane>,
    log: Arc<FactoryLog>,
    wakes: Arc<WakeCount>,
    seq: u64,
    commands: Sender<(CommandSeq, OrzmuxCommand)>,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_wheel(WheelConfig::default())
    }

    /// A harness whose backend routes the wheel by `wheel`.
    pub fn with_wheel(wheel: WheelConfig) -> Self {
        let (spawned_tx, spawned_rx) = unbounded();
        let (command_tx, command_rx) = unbounded();
        let log = Arc::new(FactoryLog::default());
        let factory = FakeFactory::new(spawned_tx, Arc::clone(&log));
        let backend = Backend::new(Box::new(factory), wheel);
        let wakes = Arc::new(WakeCount::default());
        let (gui, event_rx) = GuiLink::channel(Waker::from(Arc::clone(&wakes)));
        Self {
            event_loop: EventLoop::new(backend, command_rx, gui),
            events: event_rx,
            panes: spawned_rx,
            log,
            wakes,
            seq: 0,
            commands: command_tx,
        }
    }

    /// Sends one command and runs the loop's command and flush phases,
    /// so the events it generated are queued on the event channel.
    pub fn send(&mut self, command: OrzmuxCommand) -> CommandSeq {
        self.seq += 1;
        let seq = CommandSeq(self.seq);
        self.commands
            .send((seq, command))
            .expect("the harness holds the command receiver");
        self.event_loop.drain_commands();
        self.event_loop.flush_events();
        seq
    }

    /// Hands out every event the loop has produced, flushing the
    /// backend's outbox onto the event channel first.
    pub fn drain(&mut self) -> VecDeque<OrzmuxEvent> {
        self.event_loop.flush_events();
        self.events.try_iter().collect()
    }

    /// The backend the loop drives.
    pub fn backend(&self) -> &Backend {
        self.event_loop.backend()
    }

    /// The loop the harness drives.
    pub fn event_loop(&self) -> &EventLoop {
        &self.event_loop
    }

    /// The loop the harness drives, for a test that changes its state.
    pub fn event_loop_mut(&mut self) -> &mut EventLoop {
        &mut self.event_loop
    }

    /// The next pane the factory spawned, or `None` when none is waiting.
    pub fn spawned_pane(&self) -> Option<FakePane> {
        self.panes.try_recv().ok()
    }

    /// Makes the next spawn request fail.
    pub fn fail_next_spawn(&self) {
        self.log.fail_next.store(true, Ordering::Release);
    }

    /// Makes the next spawned pane's PTY writer fail every write.
    pub fn fail_next_writes(&self) {
        self.log.fail_writes_next.store(true, Ordering::Release);
    }

    /// Makes `sink` the PTY writer of the next spawned pane; its writes
    /// block until the test releases it.
    pub fn block_next_writes(&self, sink: BlockingSink) {
        *self.log.block_writes_next.lock().unwrap() = Some(sink);
    }

    /// Starts the output stream of every pane spawned from now on with
    /// `output`, left unread until the test pumps the pane.
    pub fn set_spawn_output(&self, output: &[u8]) {
        *self.log.spawn_output.lock().unwrap() = Some(output.to_vec());
    }

    /// The grid size of the latest spawn request, or `None` before the
    /// first.
    pub fn last_spawn_size(&self) -> Option<GridSize> {
        self.log.sizes.lock().unwrap().last().copied()
    }

    /// Forgets the working directories of the spawn requests so far.
    pub fn clear_spawn_cwds(&self) {
        self.log.cwds.lock().unwrap().clear();
    }

    /// The working directory of the latest spawn request since the last
    /// clear, or `None` when there was none or it named no directory.
    pub fn last_spawn_cwd(&self) -> Option<PathBuf> {
        self.log.cwds.lock().unwrap().last().cloned().flatten()
    }

    /// The number of wakes the loop has sent the GUI.
    pub fn wake_count(&self) -> usize {
        self.wakes.get()
    }

    /// Pumps one pane, as the loop does when its stream is ready.
    pub fn pump_pane(&mut self, id: PaneId) {
        self.event_loop.backend_mut().pump_pane(id);
    }

    /// Pumps every pane whose deadline has passed.
    pub fn service_deadlines(&mut self) {
        self.event_loop.backend_mut().service_deadlines();
    }

    /// Waits until every live pane's queued PTY writes have been
    /// written.
    pub fn settle_writes(&self) {
        for pane in self.backend().panes().map(|(_, p)| p) {
            pane.tty.settle_writes();
        }
    }

    pub fn resize(&mut self, size: GridSize) {
        self.send(OrzmuxCommand::Resize {
            size,
            cell_px: CellPixels {
                width: 8,
                height: 16,
            },
        });
    }

    pub fn open_root(&mut self) -> (PaneId, FakePane) {
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
