//! UDS msgpack control plane between daemon and cef_host.
//!
//! Frame format: 4-byte big-endian length prefix, followed by msgpack-encoded
//! `HostCommand` (daemon → cef_host) or `HostEvent` (cef_host → daemon).
//!
//! Handshake: cef_host connects, sends `HostEvent::Hello`, waits for
//! `HostCommand::Ready`. Subsequent traffic is bidirectional.
//!
//! `BrowserCreate` carries an ancillary shm fd via SCM_RIGHTS — wiring is in
//! Task 20.

use crate::pool::CefCommand;
use crate::post_command::{self, CommandQueue};
use ozmux_browser_cef_protocol::wire::{HostCommand, HostEvent};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};

/// Connects to the daemon UDS, performs the Hello/Ready handshake, then
/// forwards `HostCommand`s into the CEF command queue and pumps outgoing
/// events back to the daemon.
pub async fn run(
    socket_path: PathBuf,
    queue: CommandQueue,
    mut events_rx: mpsc::UnboundedReceiver<HostEvent>,
) -> std::io::Result<()> {
    let std_stream = StdUnixStream::connect(&socket_path)?;
    std_stream.set_nonblocking(true)?;
    let stream = UnixStream::from_std(std_stream)?;
    let (rd, wr) = stream.into_split();
    let rd = Arc::new(Mutex::new(rd));
    let wr = Arc::new(Mutex::new(wr));

    let hello = HostEvent::Hello {
        cef_version: env!("CARGO_PKG_VERSION").to_string(),
        abi_version: 1,
        pid: std::process::id(),
    };
    send_msg(&wr, &hello).await?;
    tracing::info!("sent Hello");

    let ready: HostCommand = recv_msg(&rd).await?;
    match ready {
        HostCommand::Ready { runtime_root } => {
            tracing::info!(runtime_root, "received Ready, handshake complete");
        }
        other => {
            return Err(std::io::Error::other(format!(
                "expected Ready, got {other:?}"
            )));
        }
    }

    loop {
        tokio::select! {
            cmd = recv_msg::<HostCommand>(&rd) => {
                let cmd = match cmd {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "recv failed; closing control loop");
                        break;
                    }
                };
                let internal_cmd = match cmd {
                    HostCommand::BrowserCreate { aid, initial_url, epoch } => {
                        // NOTE: shm_fd wiring via SCM_RIGHTS is Task 20; the placeholder -1
                        // makes BrowserCreate fail fast inside the pool until then.
                        CefCommand::BrowserCreate { aid, initial_url, epoch, shm_fd: -1 }
                    }
                    HostCommand::Resize { aid, css_w, css_h, dpr } => {
                        CefCommand::Resize { aid, css_w, css_h, dpr }
                    }
                    HostCommand::Close { aid } => CefCommand::Close { aid },
                    HostCommand::Shutdown => CefCommand::Shutdown,
                    HostCommand::Ready { .. } => continue,
                };
                let is_shutdown = matches!(internal_cmd, CefCommand::Shutdown);
                post_command::post(&queue, internal_cmd);
                if is_shutdown {
                    break;
                }
            }
            ev = events_rx.recv() => {
                let Some(ev) = ev else { break; };
                if let Err(e) = send_msg(&wr, &ev).await {
                    tracing::warn!(error = %e, "send event failed; closing control loop");
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn send_msg<T: serde::Serialize>(
    wr: &Arc<Mutex<OwnedWriteHalf>>,
    msg: &T,
) -> std::io::Result<()> {
    let payload = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::other("control frame too large"))?;
    let mut wr = wr.lock().await;
    wr.write_all(&len.to_be_bytes()).await?;
    wr.write_all(&payload).await?;
    wr.flush().await?;
    Ok(())
}

async fn recv_msg<T: serde::de::DeserializeOwned>(
    rd: &Arc<Mutex<OwnedReadHalf>>,
) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    let mut rd = rd.lock().await;
    rd.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    rd.read_exact(&mut payload).await?;
    rmp_serde::from_slice(&payload).map_err(std::io::Error::other)
}
