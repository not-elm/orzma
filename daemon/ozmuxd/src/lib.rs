//! The ozmuxd daemon server: a single central loop thread solely owns an
//! `ozmux_mux::Mux`, applies `ClientMessage` commands, and broadcasts the
//! resulting `MuxEvent`s to every attached client. Terminal driver lifecycle,
//! input routing, and frame fan-out are wired here (Plan 4b-2a T3).

mod surface_io;
mod transport;

pub use transport::{ServerHandle, default_socket_path};

use crossbeam_channel::{Receiver, Sender, unbounded};
use ozmux_mux::{Mux, MuxEvent, SessionId, SessionSnapshot, SurfaceId, SurfaceKind};
use ozmux_proto::{ClientMessage, ServerMessage};
use ozmux_vt::pty::Pty;
use ozmux_vt::vt::Vt;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use surface_io::{DriverCtl, spawn_driver};

/// Identifies a connected client within the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClientId(pub u64);

/// Per-client outbound queue depth; a client whose bounded control queue is
/// full (or closed) is disconnected. NEVER drop individual events — `ClientMirror`
/// is a gapless in-order fold, so a dropped event is permanent divergence.
pub(crate) const CLIENT_QUEUE_DEPTH: usize = 1024;

/// Bound on a client's in-flight frame queue. Frames are lossy: on overflow the
/// driver drops + marks the client lagged (vs the control channel which disconnects).
pub(crate) const FRAME_QUEUE_DEPTH: usize = 16;

/// The single mailbox the central loop consumes — the only serialization point.
pub(crate) enum LoopMsg {
    /// A connection finished its read of `Hello`; register + send `Welcome`.
    Attach {
        /// The client being attached.
        client_id: ClientId,
        /// Outbound sender for this client's control messages.
        writer: Sender<ServerMessage>,
        /// Outbound sender for this client's frame messages (unbounded).
        frame_writer: Sender<ServerMessage>,
        /// Initial viewport in `(cols, rows)`.
        viewport: (u16, u16),
        /// Protocol version the client claims.
        protocol_version: u32,
        /// Closes the underlying connection so the reader thread unblocks on evict
        /// or shutdown. `None` in unit tests that drive the loop with in-memory channels.
        disconnect: Option<Box<dyn FnOnce() + Send>>,
    },
    /// A decoded command from a client.
    ClientFrame(ClientId, ClientMessage),
    /// A client's connection ended.
    Disconnect(ClientId),
    /// Introspection hook: reply with the current Mux snapshot.
    Snapshot {
        /// Channel to send the snapshot back on.
        reply: Sender<SessionSnapshot>,
    },
    /// A per-surface VT event relayed from a driver (to broadcast or fold into the Mux).
    SurfaceEvent {
        /// The surface that raised it.
        surface: SurfaceId,
        /// The event.
        event: ozmux_vt::event::VtEvent,
    },
    /// Stop the loop.
    Shutdown,
}

/// A connected client's outbound channels plus a teardown hook that closes its
/// underlying connection (so the reader thread unblocks). The hook is `None`
/// in unit tests that drive the loop with in-memory channels.
struct ClientConn {
    tx: Sender<ServerMessage>,
    frame_tx: Sender<ServerMessage>,
    disconnect: Option<Box<dyn FnOnce() + Send>>,
}

/// The central loop's handle to a surface's driver thread.
struct SurfaceHandle {
    input_tx: Sender<Vec<u8>>,
    ctl_tx: Sender<DriverCtl>,
    join: Option<std::thread::JoinHandle<()>>,
}

/// The daemon server: owns the Mux + the active session (not yet listening — see `transport`, P4a T3).
pub struct Server {
    mux: Mux,
    session: SessionId,
}

/// A running central loop: the `LoopMsg` sender + the loop thread.
pub(crate) struct LoopHandle {
    tx: Sender<LoopMsg>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Creates a server using the Mux's built-in seed (session+workspace+pane).
    pub fn new() -> Self {
        let mux = Mux::new();
        let session = mux.sessions()[0];
        Self { mux, session }
    }

    /// Spawns the central loop on its own thread; returns a handle.
    pub(crate) fn spawn_loop(self) -> LoopHandle {
        let (tx, rx) = unbounded::<LoopMsg>();
        let loop_tx = tx.clone();
        let join = std::thread::spawn(move || self.run(rx, loop_tx));
        LoopHandle {
            tx,
            join: Some(join),
        }
    }

    fn run(mut self, rx: Receiver<LoopMsg>, loop_tx: Sender<LoopMsg>) {
        let mut clients: HashMap<ClientId, ClientConn> = HashMap::new();
        let mut surfaces: HashMap<SurfaceId, SurfaceHandle> = HashMap::new();

        if let Ok(snap) = self.mux.snapshot(self.session) {
            for ws in &snap.workspaces {
                for pane in &ws.panes {
                    for surf in &pane.surfaces {
                        if surf.kind == SurfaceKind::Terminal {
                            self.spawn_surface(
                                &mut surfaces,
                                &clients,
                                surf.surface,
                                cwd_opt(&surf.cwd),
                                &loop_tx,
                            );
                        }
                    }
                }
            }
        }

        while let Ok(msg) = rx.recv() {
            match msg {
                LoopMsg::Attach {
                    client_id,
                    writer,
                    frame_writer,
                    viewport,
                    protocol_version,
                    disconnect,
                } => {
                    if protocol_version != ozmux_proto::PROTOCOL_VERSION {
                        let _ = writer.try_send(ServerMessage::Error {
                            message: format!("protocol version mismatch: {protocol_version}"),
                        });
                        continue;
                    }
                    let snapshot = match self.mux.snapshot(self.session) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = writer.try_send(ServerMessage::Error {
                                message: format!("{e:?}"),
                            });
                            continue;
                        }
                    };
                    if writer
                        .try_send(ServerMessage::Welcome {
                            protocol_version: ozmux_proto::PROTOCOL_VERSION,
                            snapshot,
                        })
                        .is_err()
                    {
                        continue;
                    }
                    let conn = ClientConn {
                        tx: writer,
                        frame_tx: frame_writer,
                        disconnect,
                    };
                    clients.insert(client_id, conn);
                    let ws = self.mux.active_workspace();
                    if let Ok(events) = self.mux.set_workspace_size(ws, viewport.0, viewport.1) {
                        let evicted = broadcast(&mut clients, &events);
                        for cid in evicted {
                            for h in surfaces.values() {
                                let _ = h.ctl_tx.send(DriverCtl::RemoveClient { id: cid });
                            }
                        }
                        self.handle_mux_events(&mut surfaces, &clients, &events, &loop_tx);
                    }
                    for h in surfaces.values() {
                        if let Some(conn) = clients.get(&client_id) {
                            let _ = h.ctl_tx.send(DriverCtl::AddClient {
                                id: client_id,
                                frame_tx: conn.frame_tx.clone(),
                            });
                        }
                    }
                }
                LoopMsg::ClientFrame(_cid, ClientMessage::Input { surface, bytes }) => {
                    if let Some(h) = surfaces.get(&surface) {
                        let _ = h.input_tx.send(bytes);
                    }
                }
                LoopMsg::ClientFrame(_cid, ClientMessage::Scroll { surface, delta }) => {
                    if let Some(h) = surfaces.get(&surface) {
                        let _ = h.ctl_tx.send(DriverCtl::Scroll { delta });
                    }
                }
                LoopMsg::ClientFrame(cid, cmd) => {
                    let (evicted, events) = self.apply_command(&mut clients, cid, cmd);
                    for dead_cid in evicted {
                        for h in surfaces.values() {
                            let _ = h.ctl_tx.send(DriverCtl::RemoveClient { id: dead_cid });
                        }
                    }
                    self.handle_mux_events(&mut surfaces, &clients, &events, &loop_tx);
                }
                LoopMsg::Disconnect(cid) => {
                    if let Some(conn) = clients.remove(&cid)
                        && let Some(d) = conn.disconnect
                    {
                        d();
                    }
                    for h in surfaces.values() {
                        let _ = h.ctl_tx.send(DriverCtl::RemoveClient { id: cid });
                    }
                }
                LoopMsg::Snapshot { reply } => {
                    if let Ok(s) = self.mux.snapshot(self.session) {
                        let _ = reply.send(s);
                    }
                }
                LoopMsg::SurfaceEvent { surface, event } => match event {
                    ozmux_vt::event::VtEvent::CurrentDir(path) => {
                        if let Ok(events) = self.mux.set_surface_cwd(surface, path) {
                            let evicted = broadcast(&mut clients, &events);
                            for cid in evicted {
                                for h in surfaces.values() {
                                    let _ = h.ctl_tx.send(DriverCtl::RemoveClient { id: cid });
                                }
                            }
                            self.handle_mux_events(&mut surfaces, &clients, &events, &loop_tx);
                        }
                    }
                    other => {
                        let evicted = broadcast_messages(
                            &mut clients,
                            std::iter::once(ServerMessage::SurfaceEvent {
                                surface,
                                event: other,
                            }),
                        );
                        for cid in evicted {
                            for h in surfaces.values() {
                                let _ = h.ctl_tx.send(DriverCtl::RemoveClient { id: cid });
                            }
                        }
                    }
                },
                LoopMsg::Shutdown => {
                    for (_, h) in surfaces.drain() {
                        let _ = h.ctl_tx.send(DriverCtl::Shutdown);
                        if let Some(j) = h.join {
                            let _ = j.join();
                        }
                    }
                    for (_, conn) in clients.drain() {
                        if let Some(d) = conn.disconnect {
                            d();
                        }
                    }
                    return;
                }
            }
        }
    }

    fn apply_command(
        &mut self,
        clients: &mut HashMap<ClientId, ClientConn>,
        cid: ClientId,
        cmd: ClientMessage,
    ) -> (Vec<ClientId>, Vec<MuxEvent>) {
        let result = match cmd {
            ClientMessage::Split {
                pane,
                orientation,
                side,
                kind,
                cwd,
            } => self.mux.split_pane(pane, orientation, side, kind, cwd),
            ClientMessage::Close { pane } => self.mux.close_pane(pane),
            ClientMessage::Navigate { pane, direction } => self.mux.navigate(pane, direction),
            ClientMessage::SetActivePane { pane, .. } => self.mux.focus_pane(pane),
            ClientMessage::SpawnSurface { pane, kind, cwd } => {
                self.mux.spawn_surface(pane, kind, cwd)
            }
            ClientMessage::BreakSurfaceToPane {
                surface,
                orientation,
                side,
            } => self.mux.break_surface_to_pane(surface, orientation, side),
            ClientMessage::SetViewport { cols, rows } => {
                let ws = self.mux.active_workspace();
                self.mux.set_workspace_size(ws, cols, rows)
            }
            // NOTE: Hello is consumed at Attach before ClientFrame is queued;
            // a post-attach Hello is a client bug — ignore rather than error to
            // avoid a feedback loop if the client retransmits on reconnect.
            ClientMessage::Hello { .. } => return (vec![], vec![]),
            // NOTE: Input is handled directly in the run loop before apply_command.
            ClientMessage::Input { .. } => return (vec![], vec![]),
            // NOTE: Scroll is handled directly in the run loop (routed to the driver's ctl_tx) before apply_command.
            ClientMessage::Scroll { .. } => return (vec![], vec![]),
            ClientMessage::SetActiveSurface { pane, surface } => {
                self.mux.set_active_surface(pane, surface)
            }
            ClientMessage::CreateWorkspace { name } => match self.mux.new_workspace() {
                Ok(mut events) => {
                    if let Some(name) = name {
                        let ws = events.iter().find_map(|e| match e {
                            MuxEvent::WorkspaceCreated { workspace, .. } => Some(*workspace),
                            _ => None,
                        });
                        if let Some(ws) = ws
                            && let Ok(mut rename_events) = self.mux.rename_workspace(ws, name)
                        {
                            events.append(&mut rename_events);
                        }
                    }
                    Ok(events)
                }
                Err(e) => Err(e),
            },
            ClientMessage::SelectWorkspace { workspace } => self.mux.select_workspace(workspace),
        };
        match result {
            Ok(events) => {
                let evicted = broadcast(clients, &events);
                (evicted, events)
            }
            Err(e) => {
                if let Some(conn) = clients.get(&cid) {
                    let _ = conn.tx.try_send(ServerMessage::Error {
                        message: format!("{e:?}"),
                    });
                }
                (vec![], vec![])
            }
        }
    }

    fn handle_mux_events(
        &self,
        surfaces: &mut HashMap<SurfaceId, SurfaceHandle>,
        clients: &HashMap<ClientId, ClientConn>,
        events: &[MuxEvent],
        loop_tx: &Sender<LoopMsg>,
    ) {
        for ev in events {
            match ev {
                MuxEvent::PaneCreated {
                    surfaces: manifest, ..
                } => {
                    for e in manifest {
                        if e.kind == SurfaceKind::Terminal {
                            self.spawn_surface(
                                surfaces,
                                clients,
                                e.surface,
                                cwd_opt(&e.cwd),
                                loop_tx,
                            );
                        }
                    }
                }
                MuxEvent::SurfaceSpawned {
                    surface, kind, cwd, ..
                } if *kind == SurfaceKind::Terminal => {
                    self.spawn_surface(surfaces, clients, *surface, cwd_opt(cwd), loop_tx);
                }
                MuxEvent::SurfaceClosed { surface } => {
                    kill_surface(surfaces, *surface);
                }
                MuxEvent::PaneClosed { pane } => {
                    if let Ok(surfs) = self.mux.surfaces(*pane) {
                        for s in surfs {
                            kill_surface(surfaces, s);
                        }
                    }
                }
                MuxEvent::PaneResized { pane, cols, rows } => {
                    if let Ok(surfs) = self.mux.surfaces(*pane) {
                        for s in surfs {
                            if self.mux.surface_kind(s).ok() == Some(SurfaceKind::Terminal)
                                && let Some(h) = surfaces.get(&s)
                            {
                                let _ = h.ctl_tx.send(DriverCtl::Resize {
                                    cols: *cols,
                                    rows: *rows,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn spawn_surface(
        &self,
        surfaces: &mut HashMap<SurfaceId, SurfaceHandle>,
        clients: &HashMap<ClientId, ClientConn>,
        surface: SurfaceId,
        cwd: Option<&std::path::Path>,
        loop_tx: &Sender<LoopMsg>,
    ) {
        // NOTE: a surface has exactly one driver. break_surface_to_pane re-emits
        // the moved (already-running) surface inside its PaneCreated manifest;
        // without this guard spawn_surface would overwrite the live SurfaceHandle,
        // dropping its ctl_tx and killing the user's running shell. Skip if present.
        if surfaces.contains_key(&surface) {
            return;
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let pty = match Pty::spawn(80, 24, &shell, cwd, &[]) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("failed to spawn PTY for surface {:?}: {e}", surface);
                return;
            }
        };
        let vt = Vt::new(80, 24);
        let (input_tx, ctl_tx, join) = spawn_driver(surface, pty, vt, loop_tx.clone());
        for (id, conn) in clients {
            let _ = ctl_tx.send(DriverCtl::AddClient {
                id: *id,
                frame_tx: conn.frame_tx.clone(),
            });
        }
        surfaces.insert(
            surface,
            SurfaceHandle {
                input_tx,
                ctl_tx,
                join: Some(join),
            },
        );
    }
}

impl Default for Server {
    /// Creates a default server (same as `Server::new()`).
    fn default() -> Self {
        Self::new()
    }
}

impl LoopHandle {
    /// Sends a message to the central loop.
    #[cfg(test)]
    fn send(&self, msg: LoopMsg) {
        let _ = self.tx.send(msg);
    }

    /// Clones the `LoopMsg` sender (for the transport's accept/reader threads).
    pub(crate) fn sender(&self) -> Sender<LoopMsg> {
        self.tx.clone()
    }
}

impl Drop for LoopHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(LoopMsg::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Broadcasts `ServerMessage`s to every client's control channel; a client whose
/// bounded queue overflows is evicted (gapless invariant). Returns evicted ids.
fn broadcast_messages(
    clients: &mut HashMap<ClientId, ClientConn>,
    msgs: impl IntoIterator<Item = ServerMessage>,
) -> Vec<ClientId> {
    let mut dead: HashSet<ClientId> = HashSet::new();
    for msg in msgs {
        for (cid, conn) in clients.iter() {
            if dead.contains(cid) {
                continue;
            }
            if conn.tx.try_send(msg.clone()).is_err() {
                dead.insert(*cid);
            }
        }
    }
    let mut evicted = Vec::new();
    for cid in dead {
        if let Some(conn) = clients.remove(&cid)
            && let Some(d) = conn.disconnect
        {
            d();
        }
        evicted.push(cid);
    }
    evicted
}

/// Broadcasts a batch of events to every client as one `Events` message; a
/// client whose bounded queue is full (or closed) is dropped
/// (disconnect-on-overflow — never skip an event for a still-connected
/// client). Returns the ids of clients that were evicted.
fn broadcast(clients: &mut HashMap<ClientId, ClientConn>, events: &[MuxEvent]) -> Vec<ClientId> {
    if events.is_empty() {
        return Vec::new();
    }
    broadcast_messages(
        clients,
        std::iter::once(ServerMessage::Events(events.to_vec())),
    )
}

/// Maps a surface cwd to a spawn argument, treating the empty `PathBuf` (the
/// wire "no cwd" sentinel) as `None` so the PTY inherits the default directory.
fn cwd_opt(cwd: &Path) -> Option<&Path> {
    if cwd.as_os_str().is_empty() {
        None
    } else {
        Some(cwd)
    }
}

fn kill_surface(surfaces: &mut HashMap<SurfaceId, SurfaceHandle>, surface: SurfaceId) {
    if let Some(h) = surfaces.remove(&surface) {
        let _ = h.ctl_tx.send(DriverCtl::Shutdown);
        if let Some(j) = h.join {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{bounded, unbounded};
    use ozmux_mux::{PaneId, Side, SplitOrientation, SurfaceKind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn dummy_event(tag: u32) -> MuxEvent {
        MuxEvent::PaneResized {
            pane: PaneId::default(),
            cols: tag as u16,
            rows: 0,
        }
    }

    fn is_prefill(ev: &MuxEvent) -> bool {
        matches!(ev, MuxEvent::PaneResized { cols: 0, .. })
    }

    fn conn(tx: crossbeam_channel::Sender<ServerMessage>) -> ClientConn {
        let (frame_tx, _) = unbounded();
        ClientConn {
            tx,
            frame_tx,
            disconnect: None,
        }
    }

    fn frame_contains(frame: &ozmux_vt::frame::Frame, needle: &str) -> bool {
        use ozmux_vt::frame::{Frame, FrameDelta, FrameSnapshot};
        match frame {
            Frame::Snapshot(FrameSnapshot { rows_data, .. }) => rows_data
                .iter()
                .any(|r| r.runs.iter().any(|run| run.text.contains(needle))),
            Frame::Delta(FrameDelta { dirty_rows, .. }) => dirty_rows
                .iter()
                .any(|dr| dr.runs.iter().any(|run| run.text.contains(needle))),
        }
    }

    fn frame_display_offset(frame: &ozmux_vt::frame::Frame) -> u32 {
        use ozmux_vt::frame::Frame;
        match frame {
            Frame::Snapshot(s) => s.display_offset,
            Frame::Delta(d) => d.display_offset,
        }
    }

    fn poll_frame_until<F>(
        frame_rx: &crossbeam_channel::Receiver<ServerMessage>,
        dur: Duration,
        pred: F,
    ) -> bool
    where
        F: Fn(&ozmux_vt::frame::Frame) -> bool,
    {
        let deadline = std::time::Instant::now() + dur;
        while std::time::Instant::now() < deadline {
            match frame_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ServerMessage::Frame { frame, .. }) => {
                    if pred(&frame) {
                        return true;
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        false
    }

    #[test]
    fn broadcast_drops_overflowing_client_but_keeps_others() {
        let mut clients: HashMap<ClientId, ClientConn> = HashMap::new();
        let (full_tx, _full_rx) = bounded::<ServerMessage>(1);
        full_tx
            .try_send(ServerMessage::Events(vec![dummy_event(0)]))
            .unwrap();
        let (live_tx, live_rx) = bounded::<ServerMessage>(8);
        clients.insert(ClientId(1), conn(full_tx));
        clients.insert(ClientId(2), conn(live_tx));

        let evicted = broadcast(&mut clients, &[dummy_event(1)]);

        assert!(
            !clients.contains_key(&ClientId(1)),
            "overflowed client dropped"
        );
        assert!(
            clients.contains_key(&ClientId(2)),
            "healthy client retained"
        );
        assert!(
            matches!(live_rx.try_recv(), Ok(ServerMessage::Events(batch)) if batch.len() == 1),
            "healthy client received one Events batch of len 1"
        );
        assert!(evicted.contains(&ClientId(1)), "evicted list includes id 1");
    }

    #[test]
    fn broadcast_evicts_client_whose_queue_is_full() {
        let mut clients: HashMap<ClientId, ClientConn> = HashMap::new();
        let (tx, rx) = bounded::<ServerMessage>(1);
        tx.try_send(ServerMessage::Events(vec![dummy_event(0)]))
            .unwrap();
        clients.insert(ClientId(1), conn(tx));

        broadcast(
            &mut clients,
            &[dummy_event(1), dummy_event(2), dummy_event(3)],
        );

        assert!(
            !clients.contains_key(&ClientId(1)),
            "overflowed client dropped"
        );
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(
            count, 1,
            "only the pre-fill message is in the queue; the batch was rejected whole \
             (gapless invariant: a client that misses any event receives nothing more)"
        );
    }

    #[test]
    fn broadcast_overflow_invokes_disconnect_hook() {
        let mut clients: HashMap<ClientId, ClientConn> = HashMap::new();
        let (full_tx, _full_rx) = bounded::<ServerMessage>(1);
        full_tx
            .try_send(ServerMessage::Events(vec![dummy_event(0)]))
            .unwrap();
        let torn_down = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&torn_down);
        let (frame_tx, _) = unbounded();
        clients.insert(
            ClientId(1),
            ClientConn {
                tx: full_tx,
                frame_tx,
                disconnect: Some(Box::new(move || flag.store(true, Ordering::SeqCst))),
            },
        );

        broadcast(&mut clients, &[dummy_event(1)]);

        assert!(
            torn_down.load(Ordering::SeqCst),
            "overflow evict must invoke the disconnect hook"
        );
    }

    #[test]
    fn shutdown_invokes_disconnect_hooks() {
        let handle = Server::new().spawn_loop();
        let torn_down = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&torn_down);
        let (w_tx, _w_rx) = unbounded::<ServerMessage>();
        let (f_tx, _f_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(99),
            writer: w_tx,
            frame_writer: f_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION,
            disconnect: Some(Box::new(move || flag.store(true, Ordering::SeqCst))),
        });
        // Drain the Welcome so Attach is processed before we shut down.
        std::thread::sleep(Duration::from_millis(50));
        handle.send(LoopMsg::Shutdown);
        // Give the loop a moment to process Shutdown.
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            torn_down.load(Ordering::SeqCst),
            "Shutdown must invoke the disconnect hook for each attached client"
        );
    }

    #[test]
    fn attach_then_split_broadcasts_and_snapshot_matches() {
        let handle = Server::new().spawn_loop();
        let (w_tx, w_rx) = unbounded::<ServerMessage>();
        let (f_tx, _f_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(1),
            writer: w_tx,
            frame_writer: f_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION,
            disconnect: None,
        });

        let welcome = w_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (snapshot, pane) = match welcome {
            ServerMessage::Welcome { snapshot, .. } => {
                let p = snapshot.workspaces[0].active_pane.unwrap();
                (snapshot, p)
            }
            other => panic!("expected Welcome, got {other:?}"),
        };
        let mut mirror = ozmux_proto::ClientMirror::from_snapshot(snapshot);
        while let Ok(ServerMessage::Events(batch)) = w_rx.recv_timeout(Duration::from_millis(150)) {
            mirror.apply_events(&batch);
        }

        handle.send(LoopMsg::ClientFrame(
            ClientId(1),
            ClientMessage::Split {
                pane,
                orientation: SplitOrientation::Horizontal,
                side: Side::After,
                kind: SurfaceKind::Terminal,
                cwd: None,
            },
        ));
        while let Ok(ServerMessage::Events(batch)) = w_rx.recv_timeout(Duration::from_millis(150)) {
            mirror.apply_events(&batch);
        }

        let (s_tx, s_rx) = unbounded();
        handle.send(LoopMsg::Snapshot { reply: s_tx });
        let server_snap = s_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(mirror.to_snapshot(), server_snap);
    }

    #[test]
    fn scroll_wire_path_moves_display_offset() {
        let handle = Server::new().spawn_loop();
        let (w_tx, w_rx) = unbounded::<ServerMessage>();
        let (f_tx, f_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(1),
            writer: w_tx,
            frame_writer: f_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION,
            disconnect: None,
        });

        let welcome = w_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let surface = match welcome {
            ServerMessage::Welcome { snapshot, .. } => snapshot
                .workspaces
                .iter()
                .flat_map(|ws| ws.panes.iter())
                .flat_map(|p| p.surfaces.iter())
                .find(|s| s.kind == SurfaceKind::Terminal)
                .map(|s| s.surface)
                .expect("a Terminal surface in the welcome snapshot"),
            other => panic!("expected Welcome, got {other:?}"),
        };

        handle.send(LoopMsg::ClientFrame(
            ClientId(1),
            ClientMessage::Input {
                surface,
                bytes: b"for i in $(seq 1 50); do echo SCROLLLINE$i; done\n".to_vec(),
            },
        ));

        let saw_scrollback = poll_frame_until(&f_rx, Duration::from_secs(10), |frame| {
            frame_contains(frame, "SCROLLLINE50")
        });
        assert!(
            saw_scrollback,
            "shell output SCROLLLINE50 must appear in a frame within 10 s"
        );
        while f_rx.try_recv().is_ok() {}

        handle.send(LoopMsg::ClientFrame(
            ClientId(1),
            ClientMessage::Scroll { surface, delta: 5 },
        ));

        let moved = poll_frame_until(&f_rx, Duration::from_secs(5), |frame| {
            frame_display_offset(frame) > 0
        });
        assert!(
            moved,
            "a frame with display_offset > 0 must arrive after the Scroll wire message"
        );
    }

    #[test]
    fn protocol_version_mismatch_sends_error_and_does_not_attach() {
        let handle = Server::new().spawn_loop();
        let (w_tx, w_rx) = unbounded::<ServerMessage>();
        let (f_tx, _f_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(2),
            writer: w_tx,
            frame_writer: f_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION + 1,
            disconnect: None,
        });

        let msg = w_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            matches!(msg, ServerMessage::Error { .. }),
            "expected Error for version mismatch, got {msg:?}"
        );
        assert!(
            w_rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "no further messages after mismatch error"
        );
    }

    #[test]
    fn disconnect_removes_client_from_broadcast() {
        let handle = Server::new().spawn_loop();
        let (w_tx, w_rx) = unbounded::<ServerMessage>();
        let (f_tx, _f_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(3),
            writer: w_tx,
            frame_writer: f_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION,
            disconnect: None,
        });

        let welcome = w_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let pane = match welcome {
            ServerMessage::Welcome { snapshot, .. } => snapshot.workspaces[0].active_pane.unwrap(),
            other => panic!("expected Welcome, got {other:?}"),
        };
        while w_rx.recv_timeout(Duration::from_millis(150)).is_ok() {}

        handle.send(LoopMsg::Disconnect(ClientId(3)));

        handle.send(LoopMsg::ClientFrame(
            ClientId(3),
            ClientMessage::Split {
                pane,
                orientation: SplitOrientation::Vertical,
                side: Side::After,
                kind: SurfaceKind::Terminal,
                cwd: None,
            },
        ));

        assert!(
            w_rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "disconnected client must not receive any further events"
        );
    }

    #[test]
    #[ignore = "stress probe for batch-broadcast atomicity; concurrent, run on demand with \
                --ignored. A concurrent drainer must only ever observe COMPLETE Events batches."]
    fn broadcast_concurrent_drainer_only_observes_complete_batches() {
        const EXPECTED: usize = 20;
        let mut torn_batches = 0u64;
        for _round in 0..2000 {
            let mut clients: HashMap<ClientId, ClientConn> = HashMap::new();
            let (tx, rx) = bounded::<ServerMessage>(1);
            tx.try_send(ServerMessage::Events(vec![dummy_event(0)]))
                .unwrap();
            clients.insert(ClientId(1), conn(tx));

            let drainer = std::thread::spawn(move || {
                let mut short = 0u64;
                while let Ok(msg) = rx.recv_timeout(Duration::from_millis(50)) {
                    match msg {
                        ServerMessage::Events(batch) if batch.len() == EXPECTED => {}
                        ServerMessage::Events(batch) if batch.iter().all(is_prefill) => {}
                        _ => short += 1,
                    }
                }
                short
            });

            let many: Vec<MuxEvent> = (1..=EXPECTED as u32).map(dummy_event).collect();
            broadcast(&mut clients, &many);
            torn_batches += drainer.join().unwrap();
        }
        assert_eq!(
            torn_batches, 0,
            "a concurrent drainer observed a torn/short Events batch (regression to per-event sends)"
        );
    }
}
