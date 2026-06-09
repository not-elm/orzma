//! ozmux control-plane server: owns one `MultiPlexer` behind an async mutex and
//! fans `MuxEvent`s to every attached client over a length-prefixed
//! UDS/NamedPipe wire. One task per connection.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use ozmux_mux::{MultiPlexer, MuxEvent, MuxResult};
use ozmux_proto::{ClientMessage, MAX_MESSAGE_BYTES, ServerMessage};
use std::sync::Arc;
use tokio::io::AsyncWrite;
use tokio::sync::{Mutex, Notify, broadcast};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

mod terminal;

/// Broadcast ring capacity; a client that lags beyond this is dropped and must
/// re-attach for a fresh snapshot (see `handle_client`).
const EVENT_CHANNEL_CAPACITY: usize = 1024;

struct ServerState {
    multiplexer: Mutex<MultiPlexer>,
    events_tx: broadcast::Sender<ServerMessage>,
    shutdown: Notify,
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
            multiplexer: Mutex::new(MultiPlexer::default()),
            events_tx,
            shutdown: Notify::new(),
        });
        Ok(Self { listener, state })
    }

    /// Accepts connections until a client sends `Shutdown`, spawning one task per
    /// connection.
    pub async fn start(&self) -> anyhow::Result<()> {
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

async fn handle_client(
    stream: interprocess::local_socket::tokio::Stream,
    state: Arc<ServerState>,
) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.split();
    let mut framed_read = FramedRead::new(read_half, codec());
    let mut framed_write = FramedWrite::new(write_half, codec());

    let (mut events_rx, welcome) = {
        let mux = state.multiplexer.lock().await;
        let events_rx = state.events_tx.subscribe();
        let snapshot = mux.snapshot(mux.active_session())?;
        (events_rx, ServerMessage::Welcome { snapshot })
    };
    write_server_message(&mut framed_write, &welcome).await?;

    loop {
        tokio::select! {
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
                    Ok(server_msg) => write_server_message(&mut framed_write, &server_msg).await?,
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
        ClientMessage::Input { .. }
        | ClientMessage::Scroll { .. }
        | ClientMessage::CopyMode { .. } => {
            // TODO: data plane (Input/Scroll/CopyMode) needs an ozmux_vt frame
            // source; this server is control-plane only.
            write_server_message(
                framed_write,
                &ServerMessage::Error {
                    message: "control-plane only; needs ozmux_vt".to_string(),
                },
            )
            .await
        }
    }
}

async fn apply<W, F>(
    framed_write: &mut FramedWrite<W, LengthDelimitedCodec>,
    state: &ServerState,
    op: F,
) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    F: FnOnce(&mut MultiPlexer) -> MuxResult<Vec<MuxEvent>>,
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
                    let _ = state.events_tx.send(ServerMessage::Events(events));
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
