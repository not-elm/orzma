//! The ozmuxd daemon server: a single central loop thread solely owns an
//! `ozmux_mux::Mux`, applies `ClientMessage` commands, and broadcasts the
//! resulting `MuxEvent`s to every attached client. Control plane only
//! (frame streaming is Plan 4b; UDS transport is `transport` / P4a T3).

mod transport;

pub use transport::{ServerHandle, default_socket_path};

use crossbeam_channel::{Receiver, Sender, unbounded};
use ozmux_mux::{Mux, MuxEvent, SessionId, SessionSnapshot, Side, SurfaceKind};
use ozmux_proto::{ClientMessage, ServerMessage};
use std::collections::HashMap;

/// Identifies a connected client within the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

/// Per-client outbound queue depth; a client that backs up past this is
/// disconnected. NEVER drop individual events — `ClientMirror` is a gapless
/// in-order fold, so a dropped event is permanent divergence.
pub const CLIENT_QUEUE_DEPTH: usize = 1024;

/// The single mailbox the central loop consumes — the only serialization point.
pub enum LoopMsg {
    /// A connection finished its read of `Hello`; register + send `Welcome`.
    Attach {
        /// The client being attached.
        client_id: ClientId,
        /// Outbound sender for this client's messages.
        writer: Sender<ServerMessage>,
        /// Initial viewport in `(cols, rows)`.
        viewport: (u16, u16),
        /// Protocol version the client claims.
        protocol_version: u32,
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
    /// Stop the loop.
    Shutdown,
}

/// The daemon server: owns the Mux + the active session (not yet listening — see `transport`, P4a T3).
pub struct Server {
    mux: Mux,
    session: SessionId,
}

/// A running central loop: the `LoopMsg` sender + the loop thread.
pub struct LoopHandle {
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
    pub fn spawn_loop(self) -> LoopHandle {
        let (tx, rx) = unbounded::<LoopMsg>();
        let join = std::thread::spawn(move || self.run(rx));
        LoopHandle {
            tx,
            join: Some(join),
        }
    }

    fn run(mut self, rx: Receiver<LoopMsg>) {
        let mut clients: HashMap<ClientId, Sender<ServerMessage>> = HashMap::new();
        while let Ok(msg) = rx.recv() {
            match msg {
                LoopMsg::Attach {
                    client_id,
                    writer,
                    viewport,
                    protocol_version,
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
                    clients.insert(client_id, writer);
                    let ws = self.mux.active_workspace();
                    if let Ok(events) = self.mux.set_workspace_size(ws, viewport.0, viewport.1) {
                        broadcast(&mut clients, &events);
                    }
                }
                LoopMsg::ClientFrame(cid, cmd) => self.apply_command(&mut clients, cid, cmd),
                LoopMsg::Disconnect(cid) => {
                    clients.remove(&cid);
                }
                LoopMsg::Snapshot { reply } => {
                    if let Ok(s) = self.mux.snapshot(self.session) {
                        let _ = reply.send(s);
                    }
                }
                LoopMsg::Shutdown => return,
            }
        }
    }

    fn apply_command(
        &mut self,
        clients: &mut HashMap<ClientId, Sender<ServerMessage>>,
        cid: ClientId,
        cmd: ClientMessage,
    ) {
        let result = match cmd {
            ClientMessage::Split { pane, orientation } => {
                self.mux
                    .split_pane(pane, orientation, Side::After, SurfaceKind::Terminal)
            }
            ClientMessage::Close { pane } => self.mux.close_pane(pane),
            ClientMessage::Navigate { pane, direction } => self.mux.navigate(pane, direction),
            ClientMessage::SetActivePane { pane, .. } => self.mux.focus_pane(pane),
            ClientMessage::SpawnSurface { pane, kind } => self.mux.spawn_surface(pane, kind),
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
            ClientMessage::Hello { .. } => return,
        };
        match result {
            Ok(events) => broadcast(clients, &events),
            Err(e) => {
                if let Some(w) = clients.get(&cid) {
                    let _ = w.try_send(ServerMessage::Error {
                        message: format!("{e:?}"),
                    });
                }
            }
        }
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
    pub fn send(&self, msg: LoopMsg) {
        let _ = self.tx.send(msg);
    }

    /// Clones the `LoopMsg` sender (for the transport's accept/reader threads).
    pub fn sender(&self) -> Sender<LoopMsg> {
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

/// Broadcasts each event to every client; a client whose bounded queue is full
/// (or closed) is dropped (disconnect-on-overflow — never skip an event for a
/// still-connected client).
fn broadcast(clients: &mut HashMap<ClientId, Sender<ServerMessage>>, events: &[MuxEvent]) {
    let mut dead: Vec<ClientId> = Vec::new();
    for ev in events {
        for (cid, w) in clients.iter() {
            if dead.contains(cid) {
                continue;
            }
            if w.try_send(ServerMessage::Event(ev.clone())).is_err() {
                dead.push(*cid);
            }
        }
    }
    for cid in dead {
        clients.remove(&cid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{bounded, unbounded};
    use ozmux_mux::{PaneId, SplitOrientation};
    use std::time::Duration;

    fn dummy_event(tag: u32) -> MuxEvent {
        MuxEvent::PaneResized {
            pane: PaneId::default(),
            cols: tag as u16,
            rows: 0,
        }
    }

    #[test]
    fn broadcast_drops_overflowing_client_but_keeps_others() {
        let mut clients: HashMap<ClientId, Sender<ServerMessage>> = HashMap::new();
        let (full_tx, _full_rx) = bounded::<ServerMessage>(1);
        full_tx
            .try_send(ServerMessage::Event(dummy_event(0)))
            .unwrap();
        let (live_tx, live_rx) = bounded::<ServerMessage>(8);
        clients.insert(ClientId(1), full_tx);
        clients.insert(ClientId(2), live_tx);

        broadcast(&mut clients, &[dummy_event(1)]);

        assert!(
            !clients.contains_key(&ClientId(1)),
            "overflowed client dropped"
        );
        assert!(
            clients.contains_key(&ClientId(2)),
            "healthy client retained"
        );
        assert!(
            live_rx.try_recv().is_ok(),
            "healthy client received the event"
        );
    }

    #[test]
    fn broadcast_skips_later_events_for_an_overflowed_client() {
        let mut clients: HashMap<ClientId, Sender<ServerMessage>> = HashMap::new();
        let (tx, rx) = bounded::<ServerMessage>(2);
        clients.insert(ClientId(1), tx);

        broadcast(
            &mut clients,
            &[dummy_event(1), dummy_event(2), dummy_event(3)],
        );

        assert!(
            !clients.contains_key(&ClientId(1)),
            "overflowed client dropped"
        );
        let mut delivered = Vec::new();
        while let Ok(ServerMessage::Event(MuxEvent::PaneResized { cols, .. })) = rx.try_recv() {
            delivered.push(cols);
        }
        assert_eq!(
            delivered,
            vec![1, 2],
            "after overflow on event 3, no later event is delivered to the dropped client \
             (gapless invariant: a client that misses any event receives nothing more)"
        );
    }

    #[test]
    #[ignore = "stress probe for the within-broadcast overflow gap; concurrent, run on demand \
                with --ignored. Pre-fix this reports a nonzero gap count; post-fix it is always 0."]
    fn broadcast_overflow_never_delivers_out_of_order_under_concurrent_drain() {
        let mut total_gaps = 0u64;
        for _round in 0..2000 {
            let mut clients: HashMap<ClientId, Sender<ServerMessage>> = HashMap::new();
            let (tx, rx) = bounded::<ServerMessage>(2);
            tx.try_send(ServerMessage::Event(dummy_event(0))).unwrap();
            tx.try_send(ServerMessage::Event(dummy_event(0))).unwrap();
            clients.insert(ClientId(1), tx);

            let drainer = std::thread::spawn(move || {
                let mut seq = Vec::new();
                while let Ok(ServerMessage::Event(MuxEvent::PaneResized { cols, .. })) =
                    rx.recv_timeout(Duration::from_millis(50))
                {
                    if cols != 0 {
                        seq.push(cols);
                    }
                }
                seq
            });

            let many: Vec<MuxEvent> = (1..=20).map(dummy_event).collect();
            broadcast(&mut clients, &many);
            let seq = drainer.join().unwrap();
            if seq.windows(2).any(|w| w[1] > w[0] + 1) {
                total_gaps += 1;
            }
        }
        assert_eq!(
            total_gaps, 0,
            "a dropped client received event N+k after missing N (silent mirror corruption)"
        );
    }

    #[test]
    fn attach_then_split_broadcasts_and_snapshot_matches() {
        let handle = Server::new().spawn_loop();
        let (w_tx, w_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(1),
            writer: w_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION,
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
        while let Ok(ServerMessage::Event(ev)) = w_rx.recv_timeout(Duration::from_millis(150)) {
            mirror.apply_event(&ev);
        }

        handle.send(LoopMsg::ClientFrame(
            ClientId(1),
            ClientMessage::Split {
                pane,
                orientation: SplitOrientation::Horizontal,
            },
        ));
        while let Ok(ServerMessage::Event(ev)) = w_rx.recv_timeout(Duration::from_millis(150)) {
            mirror.apply_event(&ev);
        }

        let (s_tx, s_rx) = unbounded();
        handle.send(LoopMsg::Snapshot { reply: s_tx });
        let server_snap = s_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(mirror.to_snapshot(), server_snap);
    }

    #[test]
    fn protocol_version_mismatch_sends_error_and_does_not_attach() {
        let handle = Server::new().spawn_loop();
        let (w_tx, w_rx) = unbounded::<ServerMessage>();
        handle.send(LoopMsg::Attach {
            client_id: ClientId(2),
            writer: w_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION + 1,
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
        handle.send(LoopMsg::Attach {
            client_id: ClientId(3),
            writer: w_tx,
            viewport: (80, 24),
            protocol_version: ozmux_proto::PROTOCOL_VERSION,
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
            },
        ));

        assert!(
            w_rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "disconnected client must not receive any further events"
        );
    }
}
