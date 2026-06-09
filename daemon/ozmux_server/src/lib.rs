//! ozmux control-plane server: owns one `MultiPlexer` behind an async mutex and
//! fans `MuxEvent`s to every attached client over a length-prefixed
//! UDS/NamedPipe wire. One task per connection.

use std::sync::Arc;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use ozmux_mux::MultiPlexer;
use ozmux_proto::{ClientMessage, MAX_MESSAGE_BYTES, ServerMessage};
use tokio::io::AsyncWrite;
use tokio::sync::{Mutex, Notify, broadcast};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

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
                let Ok(bytes) = frame else { break };
                let Ok(msg) = serde_json::from_slice::<ClientMessage>(&bytes) else { continue };
                if matches!(msg, ClientMessage::Shutdown) {
                    state.shutdown.notify_one();
                    break;
                }
            }
            event = events_rx.recv() => {
                match event {
                    Ok(server_msg) => write_server_message(&mut framed_write, &server_msg).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    Ok(())
}
