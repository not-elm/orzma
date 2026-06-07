//! Bevy-free PTY I/O core: owns the PTY master, writer, blocking read
//! OS thread, child killer, and the channels the reader drains into.
//! Gated behind `feature = "pty"` so the pure VT core builds without
//! `portable-pty`.

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

/// PTY ownership for one terminal.
///
/// `Mutex` is required because `dyn MasterPty + Send` and `dyn Write +
/// Send` are `!Sync`, but the Bevy `Component` that wraps this (in
/// `bevy_terminal`) must be `Send + Sync`. `get_mut()` from a `&mut`
/// borrow is poisoning-safe (no panic-while-locked path exists).
pub struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn std::io::Write + Send>>,
    chunk_rx: Receiver<Vec<u8>>,
    exit_rx: Receiver<Option<i32>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Pty {
    /// Opens a PTY, spawns `shell` under it with the given cwd/env, starts
    /// the blocking reader thread, and returns the wired core.
    pub fn spawn(
        cols: u16,
        rows: u16,
        shell: &str,
        cwd: Option<&Path>,
        env: &[(String, String)],
    ) -> anyhow::Result<Self> {
        let pty_pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(shell);
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }

        let child = pty_pair.slave.spawn_command(cmd)?;
        let child_killer = child.clone_killer();
        drop(pty_pair.slave);

        let reader = pty_pair.master.try_clone_reader()?;
        let writer = pty_pair.master.take_writer()?;

        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (exit_tx, exit_rx) = unbounded::<Option<i32>>();

        spawn_pty_thread(reader, child, chunk_tx, exit_tx);

        Ok(Self {
            master: Mutex::new(pty_pair.master),
            writer: Mutex::new(writer),
            chunk_rx,
            exit_rx,
            child_killer,
        })
    }

    /// Resizes the PTY master.
    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master
            .get_mut()
            .expect("Pty::master mutex never poisoned")
            .resize(size)
            .map_err(|e| anyhow::anyhow!("PTY resize failed: {e}"))?;
        Ok(())
    }

    /// Writes bytes to the PTY master.
    pub fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer
            .get_mut()
            .expect("Pty::writer mutex never poisoned")
            .write_all(bytes)
    }

    /// Non-blocking poll for the next PTY chunk from the reader thread.
    pub fn try_recv_chunk(&self) -> Result<Vec<u8>, TryRecvError> {
        self.chunk_rx.try_recv()
    }

    /// Non-blocking poll for the one-shot child-exit signal. `Some(code)`
    /// on graceful exit, `None` if the wait itself failed; subsequent
    /// calls return `Err(Disconnected)`.
    pub fn try_recv_exit(&self) -> Result<Option<i32>, TryRecvError> {
        self.exit_rx.try_recv()
    }

    /// The PTY-output chunk receiver, for `select!`-driven drivers (the daemon).
    pub fn chunk_receiver(&self) -> &Receiver<Vec<u8>> {
        &self.chunk_rx
    }

    /// The child-exit one-shot receiver, for `select!`-driven drivers (the daemon).
    pub fn exit_receiver(&self) -> &Receiver<Option<i32>> {
        &self.exit_rx
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // NOTE: SIGHUP the child so the blocking reader thread's read()
        // returns EOF and exits cleanly — otherwise the OS thread leaks.
        // portable-pty's ChildKiller makes this idempotent (kill() on an
        // already-exited child is a no-op).
        let _ = self.child_killer.kill();
    }
}

/// Spawns a dedicated OS thread that drains PTY output into `chunk_tx`,
/// then sends a single `exit_tx` message when the reader returns 0 or
/// errors. The OS thread (vs. async) is required: the PTY read is blocking.
fn spawn_pty_thread(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_then_drop_is_clean() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).expect("spawn /bin/sh");
        // Dropping kills the child via the idempotent ChildKiller; this
        // asserts spawn wiring + Drop do not panic.
        drop(pty);
    }
}
