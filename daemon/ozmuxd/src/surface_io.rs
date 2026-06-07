//! Per-surface terminal driver: owns a `Pty` + `Vt`, drives the VT off PTY
//! output (a Bevy-free replica of `bevy_terminal`'s plugin drive loop), and
//! fans emitted `Frame`s to its own client set. Lifecycle + client membership
//! are controlled by the central loop via `DriverCtl`.

use crate::{ClientId, LoopMsg};
use crossbeam_channel::{Receiver, Sender, TrySendError, select, unbounded};
use ozmux_mux::SurfaceId;
use ozmux_proto::ServerMessage;
use ozmux_vt::event::VtEvent;
use ozmux_vt::frame::{Frame, SnapshotReason};
use ozmux_vt::pty::Pty;
use ozmux_vt::vt::{OutputAction, Vt};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-client frame fan-out state: the outbound sender plus whether the client
/// has fallen behind (its bounded frame channel overflowed) and needs a snapshot
/// to catch up.
struct ClientFrameState {
    frame_tx: Sender<ServerMessage>,
    lagged: bool,
}

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
    /// Scroll the VT viewport by `delta` rows (positive = into history).
    Scroll {
        /// Signed row delta (positive scrolls back into history).
        delta: i32,
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
    loop_tx: Sender<LoopMsg>,
) -> (
    Sender<Vec<u8>>,
    Sender<DriverCtl>,
    std::thread::JoinHandle<()>,
) {
    let (input_tx, input_rx) = unbounded::<Vec<u8>>();
    let (ctl_tx, ctl_rx) = unbounded::<DriverCtl>();
    let join = std::thread::spawn(move || run_driver(surface, pty, vt, input_rx, ctl_rx, loop_tx));
    (input_tx, ctl_tx, join)
}

fn run_driver(
    surface: SurfaceId,
    mut pty: Pty,
    mut vt: Vt,
    input_rx: Receiver<Vec<u8>>,
    ctl_rx: Receiver<DriverCtl>,
    loop_tx: Sender<LoopMsg>,
) {
    let mut clients: HashMap<ClientId, ClientFrameState> = HashMap::new();
    let mut last_resync: Option<Instant> = None;
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
                        fan_out(&mut clients, surface, &f);
                    }
                    let replies = vt.drain_replies();
                    if !replies.is_empty() {
                        let _ = pty.write_all(&replies);
                    }
                    relay_vt_events(surface, &mut vt, &loop_tx);
                }
                Err(_) => break,
            },
            recv(exit_rx) -> code => {
                // Drain output the PTY reader buffered before the child exited so
                // the shell's final output reaches clients instead of being lost:
                // select! may pick exit_rx while chunk_rx still holds chunks.
                let now = Instant::now();
                while let Ok(bytes) = chunk_rx.try_recv() {
                    vt.on_output(&bytes, now);
                }
                if let Some(f) = vt.emit() {
                    fan_out(&mut clients, surface, &f);
                }
                relay_vt_events(surface, &mut vt, &loop_tx);
                let exit_code = code.ok().flatten();
                let _ = loop_tx.send(LoopMsg::SurfaceEvent {
                    surface,
                    event: VtEvent::ChildExit { code: exit_code },
                });
                break;
            }
            recv(input_rx) -> msg => match msg {
                Ok(bytes) => {
                    // NOTE: note_user_input BEFORE write_all so a racing emit
                    // cycle cannot miss the flag. Mirrors handle.rs line 37.
                    vt.note_user_input();
                    if !vt.is_at_bottom() {
                        vt.scroll_to_bottom();
                    }
                    let _ = pty.write_all(&bytes);
                }
                Err(_) => break,
            },
            recv(ctl_rx) -> msg => match msg {
                Ok(DriverCtl::AddClient { id, frame_tx }) => {
                    let snap = vt.force_snapshot(SnapshotReason::Reconnect);
                    let lagged = frame_tx
                        .try_send(ServerMessage::Frame {
                            surface,
                            frame: Frame::Snapshot(snap),
                        })
                        .is_err();
                    clients.insert(id, ClientFrameState { frame_tx, lagged });
                }
                Ok(DriverCtl::RemoveClient { id }) => {
                    clients.remove(&id);
                }
                Ok(DriverCtl::Resize { cols, rows }) => {
                    vt.resize(cols, rows);
                    let _ = pty.resize(cols, rows);
                }
                Ok(DriverCtl::Scroll { delta }) => {
                    vt.scroll(delta);
                    if let Some(f) = vt.emit() {
                        fan_out(&mut clients, surface, &f);
                    }
                }
                Ok(DriverCtl::Shutdown) | Err(_) => break,
            },
            default(timeout) => {}
        }
        // NOTE: run the deadline/bootstrap flush on EVERY iteration, not just the
        // `default` arm. Under a gapless firehose `chunk_rx` is always ready, so
        // `select!` never selects `default`; an armed coalescer window would then
        // stall until output pauses (the screen freezes). `Vt::tick` is a no-op
        // when nothing is due, so calling it each iteration is cheap. This is the
        // daemon's equivalent of plugin.rs running `check_deadline_flush` every
        // Bevy frame.
        if let Some(f) = vt.tick(Instant::now()) {
            fan_out(&mut clients, surface, &f);
        }
        relay_vt_events(surface, &mut vt, &loop_tx);
        retry_lagged_clients(&mut clients, &mut last_resync, surface, &mut vt);
    }
}

/// Relays the VT's pending events to the central loop (gapless control path).
fn relay_vt_events(surface: SurfaceId, vt: &mut Vt, loop_tx: &Sender<LoopMsg>) {
    for event in vt.drain_events() {
        let _ = loop_tx.send(LoopMsg::SurfaceEvent { surface, event });
    }
}

/// Sends `frame` to every caught-up client via non-blocking `try_send`. A client
/// whose bounded channel is Full is marked `lagged` and the frame dropped (frames
/// are lossy — never block the driver, never disconnect). A lagged client is
/// skipped UNLESS `frame` is a full Snapshot, which re-establishes the delta base:
/// deliver it and clear the lag (reusing the emit's snapshot avoids a separate
/// force_snapshot). `retry_lagged_clients` catches up the rest.
fn fan_out(clients: &mut HashMap<ClientId, ClientFrameState>, surface: SurfaceId, frame: &Frame) {
    let is_snapshot = matches!(frame, Frame::Snapshot(_));
    for state in clients.values_mut() {
        if state.lagged && !is_snapshot {
            continue;
        }
        match state.frame_tx.try_send(ServerMessage::Frame {
            surface,
            frame: frame.clone(),
        }) {
            Ok(()) => {
                if is_snapshot {
                    state.lagged = false;
                }
            }
            Err(TrySendError::Full(_)) => state.lagged = true,
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// The minimum interval between lag-resync snapshot builds. A persistently-Full
/// client would otherwise force a full-grid `force_snapshot` every loop iteration.
const RESYNC_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Catches lagged clients up independently of VT output: if any client is lagged
/// and the throttle allows, builds one `force_snapshot(Lagged)` and `try_send`s it
/// to each lagged client whose channel now has room (Ok clears the lag). Decoupled
/// from `emit` so a client that fell behind during a burst still catches up after
/// the terminal goes idle.
fn retry_lagged_clients(
    clients: &mut HashMap<ClientId, ClientFrameState>,
    last_resync: &mut Option<Instant>,
    surface: SurfaceId,
    vt: &mut Vt,
) {
    if !clients.values().any(|s| s.lagged) {
        return;
    }
    let now = Instant::now();
    if let Some(t) = *last_resync
        && now.duration_since(t) < RESYNC_RETRY_INTERVAL
    {
        return;
    }
    *last_resync = Some(now);
    // NOTE: built in the SAME term-instant as the emit path (called at the loop
    // end, before the next select! can recv a chunk) so the resync snapshot's grid
    // matches the driver's delta base; force_snapshot leaves row_hashes untouched.
    let snap = Frame::Snapshot(vt.force_snapshot(SnapshotReason::Lagged));
    for state in clients.values_mut() {
        if !state.lagged {
            continue;
        }
        if state
            .frame_tx
            .try_send(ServerMessage::Frame {
                surface,
                frame: snap.clone(),
            })
            .is_ok()
        {
            state.lagged = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{bounded, unbounded};
    use ozmux_vt::frame::{DirtyRow, FrameDelta, FrameSnapshot};

    fn dummy_loop_tx() -> Sender<LoopMsg> {
        let (tx, _rx) = unbounded::<LoopMsg>();
        tx
    }

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
    fn driver_keeps_emitting_frames_under_sustained_output() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).unwrap();
        let vt = Vt::new(80, 24);
        let (input_tx, ctl_tx, _join) =
            spawn_driver(SurfaceId::default(), pty, vt, dummy_loop_tx());
        let (frame_tx, frame_rx) = unbounded();
        ctl_tx
            .send(DriverCtl::AddClient {
                id: ClientId(1),
                frame_tx,
            })
            .unwrap();
        let _ = frame_rx.recv_timeout(Duration::from_secs(3));
        // Truly gapless firehose: `yes` writes back-to-back 4 KiB chunks with
        // no inter-chunk pause, so the driver's chunk_rx is always ready.
        input_tx.send(b"yes PUMPPUMPPUMP\n".to_vec()).unwrap();
        // Drain whatever is already queued so we measure steady-state cadence.
        std::thread::sleep(Duration::from_millis(50));
        while frame_rx.try_recv().is_ok() {}
        let mut frames = 0u32;
        let window = Instant::now() + Duration::from_millis(500);
        while Instant::now() < window {
            if frame_rx.recv_timeout(Duration::from_millis(100)).is_ok() {
                frames += 1;
            }
        }
        ctl_tx.send(DriverCtl::Shutdown).unwrap();
        assert!(
            frames >= 5,
            "under sustained output frames must keep flowing (got {frames} in 500 ms); \
             a stall means the coalescer is never ticked while chunks are continuously ready"
        );
    }

    #[test]
    fn driver_echoes_input_to_a_frame() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).unwrap();
        let vt = Vt::new(80, 24);
        let (input_tx, ctl_tx, _join) =
            spawn_driver(SurfaceId::default(), pty, vt, dummy_loop_tx());
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

    #[test]
    fn fan_out_marks_full_client_lagged_and_drops() {
        let (tx, rx) = bounded::<ServerMessage>(1);
        let mut vt = Vt::new(80, 24);
        let _ = vt.on_output(b"hello\r\n", std::time::Instant::now());
        let frame = vt.emit().expect("a frame after output");
        tx.try_send(ServerMessage::Error {
            message: "fill".into(),
        })
        .unwrap();
        let mut clients = HashMap::new();
        clients.insert(
            ClientId(1),
            ClientFrameState {
                frame_tx: tx,
                lagged: false,
            },
        );
        fan_out(&mut clients, SurfaceId::default(), &frame);
        assert!(clients[&ClientId(1)].lagged, "Full → lagged");
        assert_eq!(
            rx.len(),
            1,
            "the frame was dropped, only the pre-fill remains"
        );
    }

    #[test]
    fn retry_catches_up_a_lagged_client_with_room() {
        let (tx, rx) = bounded::<ServerMessage>(4);
        let mut vt = Vt::new(80, 24);
        let _ = vt.on_output(b"world\r\n", std::time::Instant::now());
        let _ = vt.emit();
        let mut clients = HashMap::new();
        clients.insert(
            ClientId(1),
            ClientFrameState {
                frame_tx: tx,
                lagged: true,
            },
        );
        let mut last_resync = None;
        retry_lagged_clients(
            &mut clients,
            &mut last_resync,
            SurfaceId::default(),
            &mut vt,
        );
        assert!(!clients[&ClientId(1)].lagged, "Ok → lag cleared");
        assert!(matches!(
            rx.try_recv().unwrap(),
            ServerMessage::Frame {
                frame: Frame::Snapshot(_),
                ..
            }
        ));
    }

    #[test]
    fn retry_throttles_snapshot_builds() {
        let (tx, _rx) = bounded::<ServerMessage>(1);
        tx.try_send(ServerMessage::Error {
            message: "fill".into(),
        })
        .unwrap();
        let mut vt = Vt::new(80, 24);
        let mut clients = HashMap::new();
        clients.insert(
            ClientId(1),
            ClientFrameState {
                frame_tx: tx,
                lagged: true,
            },
        );
        let mut last_resync = None;
        retry_lagged_clients(
            &mut clients,
            &mut last_resync,
            SurfaceId::default(),
            &mut vt,
        );
        let first = last_resync;
        assert!(first.is_some(), "first retry stamped the throttle clock");
        retry_lagged_clients(
            &mut clients,
            &mut last_resync,
            SurfaceId::default(),
            &mut vt,
        );
        assert_eq!(
            last_resync, first,
            "second retry within the interval is throttled (clock unchanged)"
        );
        assert!(clients[&ClientId(1)].lagged, "still Full → still lagged");
    }

    #[test]
    fn scroll_ctl_moves_display_offset() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).unwrap();
        let mut vt = Vt::new(80, 24);
        // Feed enough lines to build scrollback (more than the 24-row viewport).
        let now = std::time::Instant::now();
        for i in 0..40u32 {
            let line = format!("LINE{i:03}\r\n");
            vt.on_output(line.as_bytes(), now);
        }
        // Consume pending damage so emit() can be called cleanly.
        let _ = vt.emit();

        // Confirm no scrollback yet in offset (we're at the live tail).
        vt.scroll(5);
        let frame = vt
            .emit()
            .expect("scroll must stage damage and emit a frame");
        let offset = match &frame {
            Frame::Snapshot(s) => s.display_offset,
            Frame::Delta(d) => d.display_offset,
        };
        assert!(
            offset > 0,
            "display_offset must be > 0 after scrolling into history (got {offset})"
        );
        drop(pty);
    }

    #[test]
    fn scroll_ctl_routed_via_driver() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).unwrap();
        let vt = Vt::new(80, 24);
        let (input_tx, ctl_tx, _join) =
            spawn_driver(SurfaceId::default(), pty, vt, dummy_loop_tx());
        let (frame_tx, frame_rx) = unbounded();
        ctl_tx
            .send(DriverCtl::AddClient {
                id: ClientId(1),
                frame_tx,
            })
            .unwrap();
        // Wait for the bootstrap snapshot.
        let _ = frame_rx.recv_timeout(Duration::from_secs(3));
        // Produce scrollback by printing more lines than the viewport rows.
        input_tx
            .send(b"for i in $(seq 1 50); do echo SCROLLLINE$i; done\n".to_vec())
            .unwrap();
        // Allow generous time for the shell to produce output and scrollback.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_scrollline = false;
        while Instant::now() < deadline {
            if let Ok(ServerMessage::Frame { frame, .. }) =
                frame_rx.recv_timeout(Duration::from_millis(200))
            {
                if frame_contains(&frame, "SCROLLLINE50") {
                    saw_scrollline = true;
                    break;
                }
            }
        }
        assert!(
            saw_scrollline,
            "shell output SCROLLLINE50 must appear in a frame within 10 s"
        );
        // Drain remaining frames.
        std::thread::sleep(Duration::from_millis(300));
        while frame_rx.try_recv().is_ok() {}
        // Send the Scroll control message.
        ctl_tx.send(DriverCtl::Scroll { delta: 5 }).unwrap();
        // Poll for a frame with display_offset > 0.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got_offset = false;
        while Instant::now() < deadline {
            match frame_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ServerMessage::Frame { frame, .. }) => {
                    let offset = match &frame {
                        Frame::Snapshot(s) => s.display_offset,
                        Frame::Delta(d) => d.display_offset,
                    };
                    if offset > 0 {
                        got_offset = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(
            got_offset,
            "a frame with display_offset > 0 must arrive after DriverCtl::Scroll"
        );
        ctl_tx.send(DriverCtl::Shutdown).unwrap();
    }

    #[test]
    fn input_after_scroll_snaps_to_bottom() {
        let pty = Pty::spawn(80, 24, "/bin/sh", None, &[]).unwrap();
        let vt = Vt::new(80, 24);
        let (input_tx, ctl_tx, _join) =
            spawn_driver(SurfaceId::default(), pty, vt, dummy_loop_tx());
        let (frame_tx, frame_rx) = unbounded();
        ctl_tx
            .send(DriverCtl::AddClient {
                id: ClientId(1),
                frame_tx,
            })
            .unwrap();
        // Wait for the bootstrap snapshot.
        let _ = frame_rx.recv_timeout(Duration::from_secs(3));
        // Produce scrollback by printing more lines than the viewport rows.
        input_tx
            .send(b"for i in $(seq 1 50); do echo SCROLLLINE$i; done\n".to_vec())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_scrollline = false;
        while Instant::now() < deadline {
            if let Ok(ServerMessage::Frame { frame, .. }) =
                frame_rx.recv_timeout(Duration::from_millis(200))
            {
                if frame_contains(&frame, "SCROLLLINE50") {
                    saw_scrollline = true;
                    break;
                }
            }
        }
        assert!(
            saw_scrollline,
            "shell output SCROLLLINE50 must appear in a frame within 10 s"
        );
        // Drain remaining frames so the next poll measures fresh state.
        std::thread::sleep(Duration::from_millis(300));
        while frame_rx.try_recv().is_ok() {}
        // Scroll back into history and confirm we are off the live tail.
        ctl_tx.send(DriverCtl::Scroll { delta: 5 }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got_offset = false;
        while Instant::now() < deadline {
            match frame_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ServerMessage::Frame { frame, .. }) => {
                    let offset = match &frame {
                        Frame::Snapshot(s) => s.display_offset,
                        Frame::Delta(d) => d.display_offset,
                    };
                    if offset > 0 {
                        got_offset = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(
            got_offset,
            "a frame with display_offset > 0 must arrive after DriverCtl::Scroll"
        );
        // Drain so the snap-triggered frame is the next one we observe.
        std::thread::sleep(Duration::from_millis(300));
        while frame_rx.try_recv().is_ok() {}
        // Typing while scrolled back must snap the viewport to the live tail.
        input_tx.send(b"x".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut snapped = false;
        while Instant::now() < deadline {
            match frame_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ServerMessage::Frame { frame, .. }) => {
                    let offset = match &frame {
                        Frame::Snapshot(s) => s.display_offset,
                        Frame::Delta(d) => d.display_offset,
                    };
                    if offset == 0 {
                        snapped = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(
            snapped,
            "a frame with display_offset == 0 must arrive after input (snap to live tail)"
        );
        ctl_tx.send(DriverCtl::Shutdown).unwrap();
    }

    #[test]
    fn snapshot_reuse_clears_lag_at_emit() {
        let (tx, rx) = bounded::<ServerMessage>(4);
        let mut vt = Vt::new(80, 24);
        let snap = Frame::Snapshot(vt.force_snapshot(SnapshotReason::Reconnect));
        let mut clients = HashMap::new();
        clients.insert(
            ClientId(1),
            ClientFrameState {
                frame_tx: tx,
                lagged: true,
            },
        );
        fan_out(&mut clients, SurfaceId::default(), &snap);
        assert!(
            !clients[&ClientId(1)].lagged,
            "Snapshot delivered to a lagged client clears the lag"
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            ServerMessage::Frame {
                frame: Frame::Snapshot(_),
                ..
            }
        ));
    }
}
