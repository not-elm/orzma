//! Per-surface terminal driver: owns a `Pty` + `Vt`, drives the VT off PTY
//! output (a Bevy-free replica of `bevy_terminal`'s plugin drive loop), and
//! fans emitted `Frame`s to its own client set. Lifecycle + client membership
//! are controlled by the central loop via `DriverCtl`.

use crate::ClientId;
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use ozmux_mux::SurfaceId;
use ozmux_proto::ServerMessage;
use ozmux_vt::frame::{Frame, SnapshotReason};
use ozmux_vt::pty::Pty;
use ozmux_vt::vt::{OutputAction, Vt};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Control messages from the central loop to a surface driver.
pub(crate) enum DriverCtl {
    /// Register a client: send it a forced snapshot FIRST, then fan-out to it.
    AddClient {
        /// The client being registered.
        id: ClientId,
        /// Outbound sender for frames to this client.
        frame_tx: Sender<ServerMessage>,
    },
    /// Unregister a client.
    RemoveClient {
        /// The client to remove.
        id: ClientId,
    },
    /// Resize the PTY + VT.
    Resize {
        /// New column count.
        cols: u16,
        /// New row count.
        rows: u16,
    },
    /// Stop the driver.
    Shutdown,
}

/// Spawns the driver thread for `surface`; returns the input sender, the
/// control sender, and the join handle.
pub(crate) fn spawn_driver(
    surface: SurfaceId,
    pty: Pty,
    vt: Vt,
) -> (
    Sender<Vec<u8>>,
    Sender<DriverCtl>,
    std::thread::JoinHandle<()>,
) {
    let (input_tx, input_rx) = unbounded::<Vec<u8>>();
    let (ctl_tx, ctl_rx) = unbounded::<DriverCtl>();
    let join = std::thread::spawn(move || run_driver(surface, pty, vt, input_rx, ctl_rx));
    (input_tx, ctl_tx, join)
}

fn run_driver(
    surface: SurfaceId,
    mut pty: Pty,
    mut vt: Vt,
    input_rx: Receiver<Vec<u8>>,
    ctl_rx: Receiver<DriverCtl>,
) {
    let mut clients: HashMap<ClientId, Sender<ServerMessage>> = HashMap::new();
    // NOTE: clone the receivers out of `pty` before the loop so `select!`
    // can borrow them independently of the mutable `pty` used for
    // `write_all` / `resize`. The underlying crossbeam channels stay alive
    // for the lifetime of the original `Pty` (which we own), so these
    // clones remain valid.
    let chunk_rx = pty.chunk_receiver().clone();
    let exit_rx = pty.exit_receiver().clone();
    loop {
        let timeout = vt
            .next_deadline()
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(100));
        select! {
            recv(chunk_rx) -> msg => match msg {
                Ok(bytes) => {
                    // NOTE: capture `now` per chunk (not once before the loop)
                    // so the coalescer's IDLE deadline is measured from each
                    // chunk's actual arrival time, mirroring plugin.rs line 157.
                    let now = Instant::now();
                    if matches!(vt.on_output(&bytes, now), OutputAction::EmitNow)
                        && let Some(f) = vt.emit()
                    {
                        fan_out(&clients, surface, &f);
                    }
                    let replies = vt.drain_replies();
                    if !replies.is_empty() {
                        let _ = pty.write_all(&replies);
                    }
                    let _ = vt.drain_events();
                }
                Err(_) => break,
            },
            recv(exit_rx) -> _ => break,
            recv(input_rx) -> msg => match msg {
                Ok(bytes) => {
                    // NOTE: note_user_input BEFORE write_all so a racing emit
                    // cycle cannot miss the flag. Mirrors handle.rs line 37.
                    vt.note_user_input();
                    let _ = pty.write_all(&bytes);
                }
                Err(_) => break,
            },
            recv(ctl_rx) -> msg => match msg {
                Ok(DriverCtl::AddClient { id, frame_tx }) => {
                    let snap = vt.force_snapshot(SnapshotReason::Reconnect);
                    let _ = frame_tx.send(ServerMessage::Frame {
                        surface,
                        frame: Frame::Snapshot(snap),
                    });
                    clients.insert(id, frame_tx);
                }
                Ok(DriverCtl::RemoveClient { id }) => {
                    clients.remove(&id);
                }
                Ok(DriverCtl::Resize { cols, rows }) => {
                    vt.resize(cols, rows);
                    let _ = pty.resize(cols, rows);
                }
                Ok(DriverCtl::Shutdown) | Err(_) => break,
            },
            default(timeout) => {
                if let Some(f) = vt.tick(Instant::now()) {
                    fan_out(&clients, surface, &f);
                }
                let _ = vt.drain_events();
            }
        }
    }
}

fn fan_out(clients: &HashMap<ClientId, Sender<ServerMessage>>, surface: SurfaceId, frame: &Frame) {
    for tx in clients.values() {
        let _ = tx.send(ServerMessage::Frame {
            surface,
            frame: frame.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use ozmux_vt::frame::{DirtyRow, FrameDelta, FrameSnapshot};

    fn frame_contains(frame: &Frame, needle: &str) -> bool {
        match frame {
            Frame::Snapshot(FrameSnapshot { rows_data, .. }) => rows_data
                .iter()
                .any(|r| r.runs.iter().any(|run| run.text.contains(needle))),
            Frame::Delta(FrameDelta { dirty_rows, .. }) => dirty_rows
                .iter()
                .any(|dr: &DirtyRow| dr.runs.iter().any(|run| run.text.contains(needle))),
        }
    }

    #[test]
    fn driver_echoes_input_to_a_frame() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).unwrap();
        let vt = Vt::new(80, 24);
        let (input_tx, ctl_tx, _join) = spawn_driver(SurfaceId::default(), pty, vt);
        let (frame_tx, frame_rx) = unbounded();
        ctl_tx
            .send(DriverCtl::AddClient {
                id: ClientId(1),
                frame_tx,
            })
            .unwrap();
        let first = frame_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("bootstrap snapshot within 3 s");
        assert!(
            matches!(
                first,
                ServerMessage::Frame {
                    frame: Frame::Snapshot(_),
                    ..
                }
            ),
            "first message must be a Snapshot, got {first:?}"
        );
        input_tx.send(b"printf ZZ\n".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut saw = false;
        while Instant::now() < deadline {
            if let Ok(ServerMessage::Frame { frame, .. }) =
                frame_rx.recv_timeout(Duration::from_millis(200))
            {
                if frame_contains(&frame, "ZZ") {
                    saw = true;
                    break;
                }
            }
        }
        assert!(saw, "expected the echoed 'ZZ' in a frame within 8 s");
        ctl_tx.send(DriverCtl::Shutdown).unwrap();
    }
}
