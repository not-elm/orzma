//! `PtyHandle` — Component owning the PTY master, writer, blocking
//! read OS thread, and child killer.

use bevy::ecs::component::Component;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use portable_pty::{ChildKiller, MasterPty, PtySize};
use std::io::Read;
use std::sync::Mutex;

/// Per-entity PTY ownership.
///
/// `Mutex` is required because `dyn MasterPty + Send` and `dyn Write +
/// Send` are `!Sync`, but `Component: Send + Sync + 'static`. Each
/// `.get_mut().unwrap()` from a Bevy `Query<&mut PtyHandle>` is
/// poisoning-safe (no panic-while-locked path exists).
///
/// All fields are private; construct via [`PtyHandle::new`] and
/// interact through the crate-internal API (`resize`, `write_all`,
/// `try_recv_chunk`, `try_recv_exit`). External callers may include
/// `&mut PtyHandle` in a `Query` for scheduling-order coordination,
/// but the type has no public methods because every mutator must be
/// driven through `TerminalHandle::write` / `resize` to preserve the
/// load-bearing invariants.
#[derive(Component)]
pub struct PtyHandle {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn std::io::Write + Send>>,
    chunk_rx: Receiver<Vec<u8>>,
    exit_rx: Receiver<Option<i32>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
}

impl PtyHandle {
    /// Constructs a fully wired handle from the pieces opened by
    /// [`crate::bundle::TerminalBundle::spawn`]. Not intended for
    /// direct construction by external callers.
    pub(crate) fn new(
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn std::io::Write + Send>,
        chunk_rx: Receiver<Vec<u8>>,
        exit_rx: Receiver<Option<i32>>,
        child_killer: Box<dyn ChildKiller + Send + Sync>,
    ) -> Self {
        Self {
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            chunk_rx,
            exit_rx,
            child_killer,
        }
    }

    /// Resize the PTY master.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master
            .get_mut()
            .expect("PtyHandle::master mutex never poisoned")
            .resize(size)
            .map_err(|e| anyhow::anyhow!("PTY resize failed: {e}"))?;
        Ok(())
    }

    /// Writes bytes to the PTY master.
    pub(crate) fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer
            .get_mut()
            .expect("PtyHandle::writer mutex never poisoned")
            .write_all(bytes)
    }

    /// Non-blocking poll for the next PTY chunk pushed by the OS
    /// reader thread.
    pub(crate) fn try_recv_chunk(&self) -> Result<Vec<u8>, TryRecvError> {
        self.chunk_rx.try_recv()
    }

    /// Non-blocking poll for the one-shot child-exit signal. Returns
    /// `Some(code)` on a graceful exit, `None` if the wait itself
    /// failed; subsequent calls return `Err(Disconnected)`.
    pub(crate) fn try_recv_exit(&self) -> Result<Option<i32>, TryRecvError> {
        self.exit_rx.try_recv()
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        // SIGHUP the child so the blocking reader thread's read()
        // returns EOF and exits cleanly. portable-pty's ChildKiller
        // makes this idempotent — calling kill() on an already-exited
        // child is a no-op.
        let _ = self.child_killer.kill();
    }
}

/// Spawns a dedicated OS thread that drains PTY output into
/// `chunk_tx`. Sends a single `exit_tx` message (Some(code) on
/// graceful exit, None on wait failure) when the reader returns 0 or
/// errors out.
///
/// The OS thread (vs. `tokio::spawn`) is required because the PTY
/// read syscall is blocking.
pub(crate) fn spawn_pty_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if chunk_tx.send(buf[..n].to_vec()).is_err() {
                        // Receiver dropped — the entity was despawned.
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_tx.send(code);
    });
}
