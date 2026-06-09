//! Per-surface VT/PTY driver: owns one terminal's `(Pty, Vt)` on a dedicated
//! OS thread and multiplexes PTY output, child exit, client commands, and the
//! coalescer deadline via `crossbeam::Select`.

use crate::terminal::{DriverCommand, DriverSeed, apply_copy_mode};
use crossbeam_channel::{Receiver, Select};
use ozmux_mux::SurfaceId;
use ozmux_proto::ServerMessage;
use ozmux_vt::event::VtEvent;
use ozmux_vt::frame::{Frame, SnapshotReason};
use ozmux_vt::pty::Pty;
use ozmux_vt::vt::{OutputAction, Vt};
use std::path::Path;
use std::thread;
use std::time::Instant;
use tokio::sync::broadcast;

/// Owns one terminal's `(Pty, Vt)` on a dedicated OS thread.
pub(crate) struct SurfaceDriver {
    surface: SurfaceId,
    pty: Pty,
    vt: Vt,
    cmd_rx: Receiver<DriverCommand>,
    events_tx: broadcast::Sender<ServerMessage>,
}

impl SurfaceDriver {
    /// Spawns the PTY + VT engine for `seed` on a detached driver thread. On a
    /// PTY spawn failure, logs and broadcasts `ChildExit { code: None }` so the
    /// client can mark the surface dead, and no thread is started.
    pub(crate) fn spawn(seed: DriverSeed, events_tx: broadcast::Sender<ServerMessage>) {
        let DriverSeed { surface, cols, rows, cwd, cmd_rx } = seed;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let env = vec![("TERM".to_string(), "xterm-256color".to_string())];
        let cwd_ref: Option<&Path> = cwd.as_deref();
        match Pty::spawn(cols, rows, &shell, cwd_ref, &env) {
            Ok(pty) => {
                let vt = Vt::new(cols, rows);
                let driver = SurfaceDriver { surface, pty, vt, cmd_rx, events_tx };
                thread::spawn(move || driver.run());
            }
            Err(error) => {
                tracing::error!(?error, ?surface, "PTY spawn failed");
                // NOTE: `cmd_rx` drops here, disconnecting the channel, so a later
                // `route` to this surface fails fast instead of queueing undrained.
                let _ = events_tx.send(ServerMessage::SurfaceEvent {
                    surface,
                    event: VtEvent::ChildExit { code: None },
                });
            }
        }
    }

    fn run(mut self) {
        // NOTE: Startup bootstrap: emit the Initial snapshot even for a quiet
        // shell. The Select loop blocks forever on `deadline == None` until
        // output, so the bootstrap rescue must be primed here.
        self.pump(Instant::now());
        // NOTE: clone the receivers once: each iteration's `Select` borrows
        // these locals (not `self`), so the `&mut self` calls in the loop are
        // unblocked. `crossbeam` receivers are ref-counted clones of the same
        // channel, so they receive identically to the originals.
        let chunk_rx = self.pty.chunk_receiver().clone();
        let exit_rx = self.pty.exit_receiver().clone();
        let cmd_rx = self.cmd_rx.clone();
        loop {
            let deadline = self.vt.next_deadline();

            let mut sel = Select::new();
            let chunk_idx = sel.recv(&chunk_rx);
            let exit_idx = sel.recv(&exit_rx);
            let cmd_idx = sel.recv(&cmd_rx);

            let picked = match deadline {
                Some(d) => sel.select_deadline(d),
                None => Ok(sel.select()),
            };

            // NOTE: capture `now` AFTER waking; using the pre-block timestamp
            // would make a deadline-elapsed `tick` see `now < deadline` and skip
            // the flush for a full extra loop.
            let now = Instant::now();

            // NOTE: Drop `sel` before any `&mut self` call: it borrows the
            // cloned receivers and, if a `SelectedOperation` from `select` were
            // left live, dropping it without a `recv`/`complete` would panic.
            drop(sel);

            match picked {
                Err(_) => self.pump(now),
                Ok(op) => {
                    let i = op.index();
                    if i == chunk_idx {
                        match op.recv(&chunk_rx) {
                            Ok(chunk) => {
                                if self.vt.on_output(&chunk, now) == OutputAction::EmitNow
                                    && let Some(frame) = self.vt.emit()
                                {
                                    self.send_frame(frame);
                                }
                                self.pump(now);
                            }
                            Err(_) => break,
                        }
                    } else if i == exit_idx {
                        let code = op.recv(&exit_rx).ok().flatten();
                        self.send_event(VtEvent::ChildExit { code });
                        break;
                    } else if i == cmd_idx {
                        match op.recv(&cmd_rx) {
                            Ok(cmd) => {
                                self.handle_cmd(cmd);
                                self.pump(now);
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        }
    }

    fn handle_cmd(&mut self, cmd: DriverCommand) {
        match cmd {
            DriverCommand::Input(bytes) => {
                // NOTE: set the user-input flag BEFORE the PTY write so the echo
                // chunk flushes immediately. Input and chunk handling share this
                // thread, so no racing emit can observe the flag mid-write.
                self.vt.note_user_input();
                let _ = self.pty.write_all(&bytes);
            }
            DriverCommand::Scroll(delta) => self.vt.scroll(delta),
            DriverCommand::Resize { cols, rows } => {
                let _ = self.pty.resize(cols, rows);
                self.vt.resize(cols, rows);
            }
            DriverCommand::CopyMode { op, reply } => {
                let text = apply_copy_mode(&mut self.vt, &op);
                if let Some(tx) = reply {
                    let _ = tx.send(text.unwrap_or_default());
                }
            }
            DriverCommand::Snapshot(tx) => {
                let _ = tx.send(self.vt.force_snapshot(SnapshotReason::Reconnect));
            }
        }
    }

    fn pump(&mut self, now: Instant) {
        let replies = self.vt.drain_replies();
        if !replies.is_empty() {
            let _ = self.pty.write_all(&replies);
        }
        for ev in self.vt.drain_events() {
            self.send_event(ev);
        }
        if let Some(frame) = self.vt.tick(now) {
            self.send_frame(frame);
        }
    }

    fn send_frame(&self, frame: Frame) {
        let _ = self.events_tx.send(ServerMessage::Frame { surface: self.surface, frame });
    }

    fn send_event(&self, event: VtEvent) {
        let _ = self
            .events_tx
            .send(ServerMessage::SurfaceEvent { surface: self.surface, event });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn recv_until_frame(rx: &mut broadcast::Receiver<ServerMessage>) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(ServerMessage::Frame { .. }) = rx.try_recv() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn driver_emits_initial_snapshot_for_quiet_shell() {
        let (events_tx, mut rx) = broadcast::channel(256);
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let seed = DriverSeed {
            surface: SurfaceId::default(),
            cols: 80,
            rows: 24,
            cwd: None,
            cmd_rx,
        };
        SurfaceDriver::spawn(seed, events_tx);
        assert!(
            recv_until_frame(&mut rx),
            "quiet shell must still emit an Initial snapshot via startup pump"
        );
        // Detached thread: dropping cmd_tx disconnects its command channel, so
        // the driver's Select returns and the thread exits on its own.
        drop(cmd_tx);
    }

    #[test]
    fn driver_input_produces_a_frame() {
        let (events_tx, mut rx) = broadcast::channel(256);
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let seed = DriverSeed {
            surface: SurfaceId::default(),
            cols: 80,
            rows: 24,
            cwd: None,
            cmd_rx,
        };
        SurfaceDriver::spawn(seed, events_tx);
        assert!(recv_until_frame(&mut rx), "initial frame");
        cmd_tx.send(DriverCommand::Input(b"printf hello\n".to_vec())).unwrap();
        assert!(recv_until_frame(&mut rx), "input must drive a subsequent frame");
        // Detached thread: dropping cmd_tx disconnects its command channel, so
        // the driver's Select returns and the thread exits on its own.
        drop(cmd_tx);
    }
}
