//! UDS msgpack control plane between daemon and cef_host.
//!
//! Frame format: 4-byte big-endian length prefix, followed by msgpack-encoded
//! `HostCommand` (daemon → cef_host) or `HostEvent` (cef_host → daemon).
//!
//! Handshake (Task 19 / Task 20):
//!   1. cef_host connects, sends `HostEvent::Hello`.
//!   2. daemon replies with `HostCommand::Ready`.
//!   3. daemon sends a single shm fd via SCM_RIGHTS (PoC simplification — one
//!      shm region per cef_host child; Plan 2 negotiates per-activity).
//!
//! The handshake runs synchronously on a blocking task so that SCM_RIGHTS
//! recvmsg can sit on the same fd without racing the Tokio reactor. Once the
//! shm fd is in hand, the stream is registered with Tokio and the
//! bidirectional command/event loop takes over.

use crate::pool::CefCommand;
use crate::post_command::{self, PoolHandle};
use ozmux_browser_cef_protocol::wire::{HostCommand, HostEvent};
use sendfd::RecvWithFd;
use std::io::{Read, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};

/// Connects to the daemon UDS, performs the Hello / Ready / shm-fd handshake,
/// then forwards `HostCommand`s into the CEF command queue and pumps outgoing
/// events back to the daemon.
pub async fn run(
    socket_path: PathBuf,
    handle: PoolHandle,
    events_rx: mpsc::UnboundedReceiver<HostEvent>,
) -> std::io::Result<()> {
    let (std_stream, shm_fd) = tokio::task::spawn_blocking(move || handshake(&socket_path))
        .await
        .map_err(std::io::Error::other)??;
    tracing::info!(shm_fd, "handshake complete");

    let stream = UnixStream::from_std(std_stream)?;
    let (rd, wr) = stream.into_split();
    let rd = Arc::new(Mutex::new(rd));
    let wr = Arc::new(Mutex::new(wr));

    pump(rd, wr, handle, events_rx, shm_fd).await
}

fn handshake(socket_path: &Path) -> std::io::Result<(StdUnixStream, RawFd)> {
    let mut stream = StdUnixStream::connect(socket_path)?;

    let hello = HostEvent::Hello {
        cef_version: env!("CARGO_PKG_VERSION").to_string(),
        abi_version: 1,
        pid: std::process::id(),
    };
    write_msg(&mut stream, &hello)?;
    tracing::debug!("sent Hello");

    let ready: HostCommand = read_msg(&mut stream)?;
    match ready {
        HostCommand::Ready { runtime_root } => {
            tracing::info!(runtime_root, "received Ready");
        }
        other => {
            return Err(std::io::Error::other(format!(
                "expected Ready, got {other:?}"
            )));
        }
    }

    let mut byte = [0u8; 1];
    let mut fds = [0i32; 1];
    let (n_bytes, n_fds) = stream.recv_with_fd(&mut byte, &mut fds)?;
    if n_fds == 0 {
        return Err(std::io::Error::other(
            "shm fd handshake: no ancillary fd received",
        ));
    }
    tracing::debug!(n_bytes, n_fds, "received shm fd");

    stream.set_nonblocking(true)?;
    Ok((stream, fds[0]))
}

async fn pump(
    rd: Arc<Mutex<OwnedReadHalf>>,
    wr: Arc<Mutex<OwnedWriteHalf>>,
    handle: PoolHandle,
    mut events_rx: mpsc::UnboundedReceiver<HostEvent>,
    handshake_shm_fd: RawFd,
) -> std::io::Result<()> {
    // NOTE: PoC supports a single shm region delivered at handshake time. The
    // fd is taken by the first BrowserCreate; subsequent BrowserCreate commands
    // before Plan 2 lands will be rejected with shm_fd = -1 (pool logs + skips).
    let mut pending_shm_fd: Option<RawFd> = Some(handshake_shm_fd);

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
                        let shm_fd = pending_shm_fd.take().unwrap_or(-1);
                        if shm_fd < 0 {
                            tracing::warn!(?aid, "BrowserCreate with no pending shm fd; pool will skip");
                        }
                        CefCommand::BrowserCreate { aid, initial_url, epoch, shm_fd }
                    }
                    HostCommand::Resize { aid, css_w, css_h, dpr } => {
                        CefCommand::Resize { aid, css_w, css_h, dpr }
                    }
                    HostCommand::Close { aid } => CefCommand::Close { aid },
                    HostCommand::Shutdown => CefCommand::Shutdown,
                    HostCommand::Ready { .. } => continue,
                };
                let is_shutdown = matches!(internal_cmd, CefCommand::Shutdown);
                if let Err(e) = post_command::post(&handle, internal_cmd) {
                    tracing::warn!(error = %e, "post_task failed; CEF shutting down?");
                    break;
                }
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

fn write_msg<T: serde::Serialize>(stream: &mut StdUnixStream, msg: &T) -> std::io::Result<()> {
    let payload = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::other("control frame too large"))?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

fn read_msg<T: serde::de::DeserializeOwned>(stream: &mut StdUnixStream) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    rmp_serde::from_slice(&payload).map_err(std::io::Error::other)
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
