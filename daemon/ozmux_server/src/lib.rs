//! ozmux control-plane server: owns one `MultiPlexer` behind an async mutex and
//! fans `MuxEvent`s to every attached client over a length-prefixed
//! UDS/NamedPipe wire. One task per connection.

use crate::terminal::{DriverCommand, TerminalRegistry};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use ozmux_mux::{Multiplexer, MuxEvent, MuxResult, SurfaceKind};
use ozmux_proto::{ClientMessage, CopyModeOp, MAX_MESSAGE_BYTES, ServerMessage};
use std::sync::Arc;
use tokio::io::AsyncWrite;
use tokio::sync::{Mutex, Notify, broadcast};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

mod socket;
mod terminal;
pub use socket::socket_path;

/// Broadcast ring capacity; a client that lags beyond this is dropped and must
/// re-attach for a fresh snapshot (see `handle_client`).
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Max time to spend writing one broadcast message to a client before treating
/// the client as a stalled reader and dropping it (it must re-attach). Mirrors
/// the lag-drop policy: a client that cannot keep up is disconnected rather
/// than allowed to block its own command path (including `Shutdown`).
const CLIENT_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Default terminal size for a surface spawned before any client has reported a
/// viewport (startup / zero clients); the first `PaneResized` reflows it.
const DEFAULT_TERMINAL_SIZE: (u16, u16) = (80, 24);

/// Max time to wait for a driver thread to answer a reply request (cold-attach
/// snapshot, copy-selection text). A live driver answers well under this; a
/// driver wedged on a blocking PTY write (or already gone) is bounded out so it
/// cannot hang the client.
const DRIVER_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

struct ServerState {
    multiplexer: Mutex<Multiplexer>,
    events_tx: broadcast::Sender<ServerMessage>,
    shutdown: Notify,
    terminals: TerminalRegistry,
}

/// A control-plane ozmux daemon: accepts client connections on a local socket,
/// serves multiplexer commands, and broadcasts state-change events to every
/// attached client.
pub struct OzmuxServer {
    listener: Listener,
    state: Arc<ServerState>,
}

impl OzmuxServer {
    /// Binds the local socket `socket_name` and seeds an empty multiplexer.
    pub fn new(socket_name: &str) -> anyhow::Result<Self> {
        let name = socket_name.to_ns_name::<GenericNamespaced>()?;
        let listener = ListenerOptions::new().name(name).create_tokio()?;
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let state = Arc::new(ServerState {
            multiplexer: Mutex::new(Multiplexer::default()),
            events_tx,
            shutdown: Notify::new(),
            terminals: TerminalRegistry::new(),
        });
        Ok(Self { listener, state })
    }

    /// Accepts connections until a client sends `Shutdown`, spawning one task per
    /// connection.
    pub async fn start(&self) -> anyhow::Result<()> {
        seed_initial_terminals(&self.state).await;
        loop {
            tokio::select! {
                conn = self.listener.accept() => {
                    let stream = conn?;
                    let state = self.state.clone();
                    tokio::spawn(async move {
                        if let Err(error) = handle_client(stream, state).await {
                            tracing::error!(?error);
                        }
                    });
                }
                _ = self.state.shutdown.notified() => break,
            }
        }
        Ok(())
    }
}

fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_MESSAGE_BYTES as usize)
        .new_codec()
}

async fn write_server_message<W: AsyncWrite + Unpin>(
    framed_write: &mut FramedWrite<W, LengthDelimitedCodec>,
    msg: &ServerMessage,
) -> anyhow::Result<()> {
    let body = serde_json::to_vec(msg)?;
    framed_write.send(Bytes::from(body)).await?;
    Ok(())
}

/// Writes `msg` to a client bounded by `CLIENT_WRITE_TIMEOUT`. Returns
/// `Ok(false)` when the write stalls (a non-draining reader) so the caller can
/// drop the connection — mirrors the lag-drop policy. Real I/O errors propagate.
async fn write_to_client<W: AsyncWrite + Unpin>(
    framed_write: &mut FramedWrite<W, LengthDelimitedCodec>,
    msg: &ServerMessage,
) -> anyhow::Result<bool> {
    match tokio::time::timeout(
        CLIENT_WRITE_TIMEOUT,
        write_server_message(framed_write, msg),
    )
    .await
    {
        Ok(result) => result.map(|()| true),
        Err(_elapsed) => Ok(false),
    }
}

async fn handle_client(
    stream: interprocess::local_socket::tokio::Stream,
    state: Arc<ServerState>,
) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.split();
    let mut framed_read = FramedRead::new(read_half, codec());
    let mut framed_write = FramedWrite::new(write_half, codec());

    let (mut events_rx, welcome, term_surfaces) = {
        let mux = state.multiplexer.lock().await;
        let events_rx = state.events_tx.subscribe();
        let snapshot = mux.snapshot(mux.active_session())?;
        let term_surfaces = terminal_surfaces(&snapshot);
        (
            events_rx,
            ServerMessage::Welcome { snapshot },
            term_surfaces,
        )
    };
    if !write_to_client(&mut framed_write, &welcome).await? {
        return Ok(());
    }

    // NOTE: subscribe (above, inside the lock) happens BEFORE these snapshot
    // requests, so every delta with seq >= the snapshot's seq is already
    // buffered in `events_rx`; the client reconciles by seq, so socket arrival
    // order does not matter. Send each snapshot ONLY to this client.
    let mut snap_reqs = Vec::new();
    for surface in term_surfaces {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if state.terminals.route(surface, DriverCommand::Snapshot(tx)) {
            snap_reqs.push((surface, rx));
        }
    }
    for (surface, rx) in snap_reqs {
        // NOTE: a driver wedged on a blocking PTY write (or already gone) never
        // answers; bound the wait so it cannot hang the attach. A live driver
        // answers in well under this budget.
        if let Ok(Ok(snapshot)) = tokio::time::timeout(DRIVER_REPLY_TIMEOUT, rx).await {
            let frame = ServerMessage::Frame {
                surface,
                frame: ozmux_vt::frame::Frame::Snapshot(snapshot),
            };
            if !write_to_client(&mut framed_write, &frame).await? {
                return Ok(());
            }
        }
    }

    loop {
        tokio::select! {
            biased;
            inbound = framed_read.next() => {
                let Some(frame) = inbound else { break };
                let bytes = match frame {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::warn!(?error, "frame decode error; closing connection");
                        break;
                    }
                };
                let Ok(msg) = serde_json::from_slice::<ClientMessage>(&bytes) else {
                    tracing::warn!("dropping malformed ClientMessage frame");
                    continue;
                };
                if matches!(msg, ClientMessage::Shutdown) {
                    state.shutdown.notify_one();
                    break;
                }
                dispatch(&mut framed_write, &state, msg).await?;
            }
            event = events_rx.recv() => {
                match event {
                    Ok(server_msg) => {
                        if !write_to_client(&mut framed_write, &server_msg).await? {
                            tracing::warn!(
                                "client write stalled past the timeout; dropping connection (client must re-attach)"
                            );
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped,
                            "client lagged past the event buffer; dropping connection (client must re-attach)"
                        );
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    Ok(())
}

async fn dispatch<W: AsyncWrite + Unpin>(
    framed_write: &mut FramedWrite<W, LengthDelimitedCodec>,
    state: &ServerState,
    msg: ClientMessage,
) -> anyhow::Result<()> {
    match msg {
        ClientMessage::Split {
            pane,
            orientation,
            side,
            kind,
            cwd,
        } => {
            apply(framed_write, state, |m| {
                m.split_pane(pane, orientation, side, kind, cwd)
            })
            .await
        }
        ClientMessage::Close { pane } => apply(framed_write, state, |m| m.close_pane(pane)).await,
        ClientMessage::Navigate { pane, direction } => {
            apply(framed_write, state, |m| m.navigate(pane, direction)).await
        }
        ClientMessage::SetActivePane { pane, .. } => {
            apply(framed_write, state, |m| m.focus_pane(pane)).await
        }
        ClientMessage::SetActiveSurface { surface } => {
            apply(framed_write, state, |m| {
                m.set_active_surface_by_surface(surface)
            })
            .await
        }
        ClientMessage::SwapPane { pane, offset } => {
            apply(framed_write, state, |m| m.swap_pane(pane, offset)).await
        }
        ClientMessage::SpawnSurface { pane, kind, cwd } => {
            apply(framed_write, state, |m| m.spawn_surface(pane, kind, cwd)).await
        }
        ClientMessage::BreakSurfaceToPane {
            surface,
            orientation,
            side,
        } => {
            apply(framed_write, state, |m| {
                m.break_surface_to_pane(surface, orientation, side)
            })
            .await
        }
        ClientMessage::SelectWorkspace { workspace } => {
            apply(framed_write, state, |m| m.select_workspace(workspace)).await
        }
        ClientMessage::SetViewport { cols, rows } => {
            apply(framed_write, state, |m| {
                let workspace = m.active_workspace();
                m.set_workspace_size(workspace, cols, rows)
            })
            .await
        }
        ClientMessage::CreateWorkspace { name } => {
            apply(framed_write, state, |m| m.new_workspace(name)).await
        }
        ClientMessage::Health => Ok(()),
        // NOTE: Shutdown is intercepted in handle_client before dispatch runs;
        // this arm only keeps the match exhaustive.
        ClientMessage::Shutdown => Ok(()),
        ClientMessage::Input { surface, bytes } => {
            state.terminals.route(surface, DriverCommand::Input(bytes));
            Ok(())
        }
        ClientMessage::Scroll { surface, delta } => {
            state.terminals.route(surface, DriverCommand::Scroll(delta));
            Ok(())
        }
        ClientMessage::CopyMode { surface, op } => {
            if matches!(op, CopyModeOp::CopySelection) {
                let (tx, rx) = tokio::sync::oneshot::channel();
                if state.terminals.route(
                    surface,
                    DriverCommand::CopyMode {
                        op,
                        reply: Some(tx),
                    },
                ) {
                    // NOTE: bound the wait — a wedged or already-gone driver never replies.
                    if let Ok(Ok(text)) = tokio::time::timeout(DRIVER_REPLY_TIMEOUT, rx).await {
                        write_server_message(
                            framed_write,
                            &ServerMessage::SelectionCopied { surface, text },
                        )
                        .await?;
                    }
                }
                Ok(())
            } else {
                state
                    .terminals
                    .route(surface, DriverCommand::CopyMode { op, reply: None });
                Ok(())
            }
        }
    }
}

/// Reserves routing slots for Terminal surfaces created by `events`, returning
/// the seeds to spawn AFTER the structural broadcast. Tears down closed
/// surfaces and resizes panes (order-independent; done before the broadcast).
fn reconcile_before_broadcast(
    state: &ServerState,
    mux: &Multiplexer,
    events: &[MuxEvent],
) -> Vec<crate::terminal::DriverSeed> {
    let mut seeds = Vec::new();
    for event in events {
        match event {
            MuxEvent::PaneCreated { pane, surfaces, .. } => {
                let (cols, rows) = mux
                    .resolved_pane_size(*pane)
                    .unwrap_or(DEFAULT_TERMINAL_SIZE);
                for entry in surfaces {
                    if entry.kind == SurfaceKind::Terminal
                        && let Some(seed) =
                            state
                                .terminals
                                .reserve(entry.surface, cols, rows, entry.cwd.clone())
                    {
                        seeds.push(seed);
                    }
                }
            }
            MuxEvent::SurfaceSpawned {
                pane,
                surface,
                kind,
                cwd,
            } if *kind == SurfaceKind::Terminal => {
                let (cols, rows) = mux
                    .resolved_pane_size(*pane)
                    .unwrap_or(DEFAULT_TERMINAL_SIZE);
                if let Some(seed) = state.terminals.reserve(*surface, cols, rows, cwd.clone()) {
                    seeds.push(seed);
                }
            }
            MuxEvent::SurfaceClosed { surface } => state.terminals.remove(*surface),
            MuxEvent::PaneResized { pane, cols, rows } => {
                if let Ok(surfaces) = mux.surfaces(*pane) {
                    for surface in surfaces {
                        state.terminals.route(
                            surface,
                            DriverCommand::Resize {
                                cols: *cols,
                                rows: *rows,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    seeds
}

/// Spawns drivers for the Terminal surfaces present in the seeded multiplexer
/// (startup / no clients). No broadcast-ordering concern: no client is attached.
async fn seed_initial_terminals(state: &Arc<ServerState>) {
    let mux = state.multiplexer.lock().await;
    let Ok(snapshot) = mux.snapshot(mux.active_session()) else {
        return;
    };
    let mut seeds = Vec::new();
    for ws in &snapshot.workspaces {
        for pane in &ws.panes {
            let (cols, rows) = mux
                .resolved_pane_size(pane.pane)
                .unwrap_or(DEFAULT_TERMINAL_SIZE);
            for surf in &pane.surfaces {
                if surf.kind == SurfaceKind::Terminal
                    && let Some(seed) =
                        state
                            .terminals
                            .reserve(surf.surface, cols, rows, surf.cwd.clone())
                {
                    seeds.push(seed);
                }
            }
        }
    }
    for seed in seeds {
        state.terminals.spawn(seed, state.events_tx.clone());
    }
}

/// Collects the Terminal-kind surfaces from a session snapshot (the surfaces
/// that have a driver and thus a frame to resnapshot on cold-attach).
fn terminal_surfaces(snapshot: &ozmux_mux::SessionSnapshot) -> Vec<ozmux_mux::SurfaceId> {
    let mut out = Vec::new();
    for ws in &snapshot.workspaces {
        for pane in &ws.panes {
            for surf in &pane.surfaces {
                if surf.kind == SurfaceKind::Terminal {
                    out.push(surf.surface);
                }
            }
        }
    }
    out
}

async fn apply<W, F>(
    framed_write: &mut FramedWrite<W, LengthDelimitedCodec>,
    state: &ServerState,
    op: F,
) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    F: FnOnce(&mut Multiplexer) -> MuxResult<Vec<MuxEvent>>,
{
    // NOTE: the broadcast send MUST stay inside this lock scope — publishing the
    // events while the mutating command still holds the mux lock is the
    // cold-attach gapless invariant. Only the error message escapes the block, so
    // the per-client error write happens without holding the lock across an await.
    let error = {
        let mut mux = state.multiplexer.lock().await;
        match op(&mut mux) {
            Ok(events) => {
                if !events.is_empty() {
                    // NOTE: reserve routing slots + teardown/resize BEFORE the broadcast so a
                    // client that sees a new surface can immediately route to it; spawn driver
                    // threads AFTER so their first frame follows the structural event.
                    let seeds = reconcile_before_broadcast(state, &mux, &events);
                    let _ = state.events_tx.send(ServerMessage::Events(events));
                    for seed in seeds {
                        state.terminals.spawn(seed, state.events_tx.clone());
                    }
                }
                None
            }
            Err(error) => Some(error.to_string()),
        }
    };
    if let Some(message) = error {
        write_server_message(framed_write, &ServerMessage::Error { message }).await?;
    }
    Ok(())
}
