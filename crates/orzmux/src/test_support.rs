//! Test fixtures for the multiplexer: PTY-less panes, a factory that
//! hands the test their input ends, and a harness that drives one
//! event loop.

use crate::backend::pane::PaneFactory;
use crate::backend::{Backend, CommandSeq, NewPaneAt, OrzmuxEvent, PaneId, RequestId};
use crate::error::OrzmuxResult;
use crate::event_loop::{EventLoop, OrzmuxCommand};
use crossbeam_channel::{Receiver, Sender, unbounded};
use orzma_tty::prelude::{OrzmaTty, OrzmaTtyError, WheelConfig};
use orzma_tty::test_support::{BlockingSink, CaptureSink, FailingSink};
use orzma_tty::{CellPixels, EnvKey, EnvValue};
use orzma_vt::prelude::{GridSize, OrzmaVt};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The test's ends of one spawned pane's streams.
pub(crate) struct FakePane {
    pub(crate) chunk_tx: Sender<Vec<u8>>,
    pub(crate) exit_tx: Sender<Option<i32>>,
    pub(crate) sink: CaptureSink,
}

/// What the factory recorded, shared with the harness through `Arc`.
#[derive(Default)]
pub(crate) struct FactoryLog {
    pub(crate) fail_next: AtomicBool,
    /// Makes the next spawned pane's PTY writer fail every write.
    pub(crate) fail_writes_next: AtomicBool,
    /// When set, the next spawned pane's PTY writer is this sink, whose
    /// writes block until it is released, as if the pane's application
    /// stopped reading stdin.
    pub(crate) block_writes_next: Mutex<Option<BlockingSink>>,
    /// When set, every spawned pane's output stream starts with these
    /// bytes, left unread until the test pumps the pane.
    pub(crate) spawn_output: Mutex<Option<Vec<u8>>>,
    pub(crate) sizes: Mutex<Vec<GridSize>>,
    pub(crate) cwds: Mutex<Vec<Option<PathBuf>>>,
}

/// Spawns PTY-less terminals and hands the test their input ends.
pub(crate) struct FakeFactory {
    pub(crate) spawned: Sender<FakePane>,
    pub(crate) log: Arc<FactoryLog>,
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

/// Drives one [`EventLoop`] whose panes are spawned by a PTY-less
/// factory.
pub(crate) struct Harness {
    pub(crate) event_loop: EventLoop,
    pub(crate) events: Receiver<OrzmuxEvent>,
    pub(crate) panes: Receiver<FakePane>,
    pub(crate) log: Arc<FactoryLog>,
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
        let (event_tx, event_rx) = unbounded();
        let log = Arc::new(FactoryLog::default());
        let factory = FakeFactory {
            spawned: spawned_tx,
            log: Arc::clone(&log),
        };
        let backend = Backend::new(Box::new(factory), wheel);
        Self {
            event_loop: EventLoop::new(backend, command_rx, event_tx),
            events: event_rx,
            panes: spawned_rx,
            log,
            seq: 0,
            commands: command_tx,
        }
    }

    /// Sends one command and runs the loop's command + flush phases,
    /// so the events it generated are queued on the event channel.
    pub fn send(&mut self, command: OrzmuxCommand) -> CommandSeq {
        self.seq += 1;
        let seq = CommandSeq(self.seq);
        let _ = self.commands.send((seq, command));
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

    /// Pumps one pane, as the loop does when its stream is ready.
    pub fn pump_pane(&mut self, id: PaneId) {
        self.event_loop.pump_pane(id);
    }

    /// Pumps every pane whose deadline has passed.
    pub fn service_deadlines(&mut self) {
        self.event_loop.service_deadlines();
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
