//! Test fixtures for the multiplexer: PTY-less panes, a factory that
//! hands the test their input ends, and a harness that drives one
//! backend.

use crate::backend::pane::PaneFactory;
use crate::backend::{Backend, CommandSeq, NewPaneAt, OrzmuxEvent, PaneId, RequestId};
use crate::error::OrzmuxResult;
use crate::protocol::OrzmuxCommand;
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

/// Drives one [`Backend`] whose panes are spawned by a PTY-less factory.
pub(crate) struct Harness {
    pub(crate) backend: Backend,
    pub(crate) events: Receiver<OrzmuxEvent>,
    pub(crate) panes: Receiver<FakePane>,
    pub(crate) log: Arc<FactoryLog>,
    pub(crate) seq: u64,
    /// Held so the command channel stays connected.
    pub(crate) _commands: Sender<(CommandSeq, OrzmuxCommand)>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        Self::with_wheel(WheelConfig::default())
    }

    /// A harness whose backend routes the wheel by `wheel`.
    pub(crate) fn with_wheel(wheel: WheelConfig) -> Self {
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

    pub(crate) fn send(&mut self, command: OrzmuxCommand) -> CommandSeq {
        self.seq += 1;
        let seq = CommandSeq(self.seq);
        self.backend.handle_command(seq, command);
        seq
    }

    pub(crate) fn drain(&self) -> VecDeque<OrzmuxEvent> {
        self.events.try_iter().collect()
    }

    /// Waits until every live pane's queued PTY writes have been
    /// written.
    pub(crate) fn settle_writes(&self) {
        for pane in self.backend.panes().map(|(_, p)| p) {
            pane.tty.settle_writes();
        }
    }

    pub(crate) fn resize(&mut self, size: GridSize) {
        self.send(OrzmuxCommand::Resize {
            size,
            cell_px: CellPixels {
                width: 8,
                height: 16,
            },
        });
    }

    pub(crate) fn open_root(&mut self) -> (PaneId, FakePane) {
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
