//! The GUI-side handle on the multiplexer backend thread: sends
//! commands, drains events, and joins the thread on drop.

use crate::backend::{Backend, ShellFactory};
use crate::protocol::{CommandSeq, OrzmuxCommand, OrzmuxEvent};
use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

/// What the backend needs to spawn shells.
#[derive(Debug, Clone)]
pub struct OrzmuxConfig {
    /// Shell override; `None` falls back to `$SHELL`, then `/bin/sh`.
    pub shell: Option<String>,
    /// Scrollback rows every pane retains on its primary screen.
    pub scrollback_rows: usize,
}

/// The backend thread could not be started.
#[derive(Debug)]
pub struct OrzmuxSpawnError(pub std::io::Error);

impl std::fmt::Display for OrzmuxSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to start the orzma-mux thread: {}", self.0)
    }
}

impl std::error::Error for OrzmuxSpawnError {}

/// The GUI's connection to the backend.
///
/// Dropping it closes the command channel, which ends the backend loop
/// (killing every pane's child), then joins the thread.
pub struct OrzmuxClient {
    commands: Option<Sender<(CommandSeq, OrzmuxCommand)>>,
    events: Receiver<OrzmuxEvent>,
    next_seq: AtomicU64,
    disconnected: AtomicBool,
    thread: Option<JoinHandle<()>>,
}

impl OrzmuxClient {
    /// Starts the backend thread (named `orzma-mux`) and returns the
    /// client connected to it.
    pub fn spawn(config: OrzmuxConfig) -> Result<Self, OrzmuxSpawnError> {
        let (command_tx, command_rx) = unbounded::<(CommandSeq, OrzmuxCommand)>();
        let (event_tx, event_rx) = unbounded::<OrzmuxEvent>();
        let factory = ShellFactory::new(config.shell, config.scrollback_rows);
        let thread = thread::Builder::new()
            .name("orzma-mux".to_string())
            .spawn(move || Backend::new(Box::new(factory), command_rx, event_tx).run())
            .map_err(OrzmuxSpawnError)?;
        Ok(Self {
            commands: Some(command_tx),
            events: event_rx,
            next_seq: AtomicU64::new(1),
            disconnected: AtomicBool::new(false),
            thread: Some(thread),
        })
    }

    /// Sends a command and returns its position in the send order. When
    /// the backend is gone the command is dropped with a warning and the
    /// returned sequence is the one that would have been used.
    pub fn send(&self, command: OrzmuxCommand) -> CommandSeq {
        let seq = CommandSeq(self.next_seq.fetch_add(1, Ordering::Relaxed));
        let sent = self
            .commands
            .as_ref()
            .is_some_and(|tx| tx.send((seq, command)).is_ok());
        if !sent {
            self.disconnected.store(true, Ordering::Release);
            tracing::warn!(?seq, "mux backend is gone; command dropped");
        }
        seq
    }

    /// Drains every event queued right now. Stops at an empty channel;
    /// a disconnected backend is recorded (see [`Self::is_disconnected`])
    /// instead of being mistaken for an empty queue.
    pub fn try_iter(&self) -> impl Iterator<Item = OrzmuxEvent> + '_ {
        std::iter::from_fn(move || match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.disconnected.store(true, Ordering::Release);
                None
            }
        })
    }

    /// Whether the backend thread has gone away (its event sender
    /// dropped, or a command send failed).
    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    /// A client with no thread: the test holds the backend's ends of
    /// both channels.
    #[cfg(any(test, feature = "test-support"))]
    pub fn detached() -> (
        Self,
        Sender<OrzmuxEvent>,
        Receiver<(CommandSeq, OrzmuxCommand)>,
    ) {
        let (command_tx, command_rx) = unbounded::<(CommandSeq, OrzmuxCommand)>();
        let (event_tx, event_rx) = unbounded::<OrzmuxEvent>();
        let client = Self {
            commands: Some(command_tx),
            events: event_rx,
            next_seq: AtomicU64::new(1),
            disconnected: AtomicBool::new(false),
            thread: None,
        };
        (client, event_tx, command_rx)
    }
}

impl Drop for OrzmuxClient {
    fn drop(&mut self) {
        drop(self.commands.take());
        if let Some(thread) = self.thread.take()
            && let Err(payload) = thread.join()
        {
            tracing::error!(?payload, "orzma-mux thread panicked");
        }
    }
}
