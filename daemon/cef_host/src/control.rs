//! UDS msgpack control plane between daemon and cef_host.
//!
//! Frame format: 4-byte big-endian length prefix, followed by msgpack-encoded
//! `HostCommand` (daemon → cef_host) or `HostEvent` (cef_host → daemon).
//!
//! Handshake (Task 19 / Task 20):
//!   1. cef_host connects, sends `HostEvent::Hello`.
//!   2. daemon replies with `HostCommand::Ready`.
//!
//! Per-BrowserCreate shm fds arrive via SCM_RIGHTS on the sendmsg carrying
//! the `BrowserCreate` body (Task A5). The hello-time single-fd hack from
//! Plan 1 has been removed.
//!
//! The handshake runs synchronously on a blocking task so that the sync I/O
//! path can sit on the same fd without racing the Tokio reactor. Once the
//! handshake is complete the stream is registered with Tokio and the
//! bidirectional command/event loop takes over.

use crate::pool::CefCommand;
use crate::post_command::{self, PoolHandle};
use ozmux_browser_cef_protocol::wire::{HostCommand, HostEvent};
use sendfd::RecvWithFd;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};

/// Closes a stray ancillary fd received with a non-`BrowserCreate` command.
/// Logs a warning naming the carrier so spurious SCM_RIGHTS bugs are visible.
fn close_stray_fd(fd: RawFd, command: &str) {
    // SAFETY: `fd` was received via recvmsg+SCM_RIGHTS inside
    // recv_command_with_fd; no other variable holds a copy of it.
    unsafe {
        libc::close(fd);
    }
    tracing::warn!(command, "stray fd received on non-BrowserCreate command, closed");
}

/// Connects to the daemon UDS, performs the Hello / Ready handshake,
/// then forwards `HostCommand`s into the CEF command queue and pumps outgoing
/// events back to the daemon.
pub async fn run(
    socket_path: PathBuf,
    handle: PoolHandle,
    events_rx: mpsc::UnboundedReceiver<HostEvent>,
) -> std::io::Result<()> {
    let std_stream = tokio::task::spawn_blocking(move || handshake(&socket_path))
        .await
        .map_err(std::io::Error::other)??;
    tracing::info!("handshake complete");

    let stream = UnixStream::from_std(std_stream)?;
    let (rd, wr) = stream.into_split();
    let rd = Arc::new(Mutex::new(rd));
    let wr = Arc::new(Mutex::new(wr));

    pump(rd, wr, handle, events_rx).await
}

fn handshake(socket_path: &Path) -> std::io::Result<StdUnixStream> {
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

    stream.set_nonblocking(true)?;
    Ok(stream)
}

async fn pump(
    rd: Arc<Mutex<OwnedReadHalf>>,
    wr: Arc<Mutex<OwnedWriteHalf>>,
    handle: PoolHandle,
    mut events_rx: mpsc::UnboundedReceiver<HostEvent>,
) -> std::io::Result<()> {
    loop {
        tokio::select! {
            cmd = recv_command_with_fd(&rd) => {
                let (cmd, fd) = match cmd {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(error = %e, "recv failed; closing control loop");
                        break;
                    }
                };
                let internal_cmd = match cmd {
                    HostCommand::BrowserCreate { aid, initial_url, epoch, cookies, profile } => {
                        let shm_fd = fd.unwrap_or(-1);
                        if shm_fd < 0 {
                            tracing::warn!(?aid, "BrowserCreate without ancillary shm_fd");
                        }
                        CefCommand::BrowserCreate { aid, initial_url, epoch, shm_fd, cookies, profile }
                    }
                    HostCommand::Resize { aid, css_w, css_h, dpr } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "Resize");
                        }
                        CefCommand::Resize { aid, css_w, css_h, dpr }
                    }
                    HostCommand::Close { aid } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "Close");
                        }
                        CefCommand::Close { aid }
                    }
                    HostCommand::Shutdown => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "Shutdown");
                        }
                        CefCommand::Shutdown
                    }
                    HostCommand::Ready { .. } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "Ready");
                        }
                        continue;
                    }
                    HostCommand::SendInput { aid, input } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "SendInput");
                        }
                        CefCommand::SendInput { aid, event: input }
                    }
                    HostCommand::Navigate { aid, url } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "Navigate");
                        }
                        CefCommand::Navigate { aid, url }
                    }
                    HostCommand::NavigateHistory { aid, delta } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "NavigateHistory");
                        }
                        CefCommand::NavigateHistory { aid, delta }
                    }
                    HostCommand::PauseScreencast { aid } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "PauseScreencast");
                        }
                        CefCommand::PauseScreencast { aid }
                    }
                    HostCommand::ResumeScreencast { aid } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "ResumeScreencast");
                        }
                        CefCommand::ResumeScreencast { aid }
                    }
                    // TODO: implement RecreateShm, GetSelection, SetClipboard.
                    HostCommand::RecreateShm { .. }
                    | HostCommand::GetSelection { .. }
                    | HostCommand::SetClipboard { .. } => {
                        if let Some(stray) = fd {
                            close_stray_fd(stray, "unimplemented");
                        }
                        tracing::warn!("unimplemented HostCommand received; ignoring");
                        continue;
                    }
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

/// Receives one `HostCommand` plus an optional ancillary `RawFd`.
///
/// The 4-byte length prefix is read asynchronously; the body and any
/// SCM_RIGHTS fd are received via `spawn_blocking` + `sendfd::RecvWithFd`
/// because `tokio::net::unix::OwnedReadHalf` does not expose `recvmsg`.
async fn recv_command_with_fd(
    rd: &Arc<Mutex<OwnedReadHalf>>,
) -> std::io::Result<(HostCommand, Option<RawFd>)> {
    let mut len_buf = [0u8; 4];
    {
        let mut g = rd.lock().await;
        g.read_exact(&mut len_buf).await?;
    }
    let len = u32::from_be_bytes(len_buf) as usize;

    // NOTE: the read half is only consumed by this function (pump's
    // sole reader), so re-acquiring `rd.lock()` after dropping it for the
    // async `read_exact` is race-free even though a small gap exists between
    // the two acquisitions. If a second reader is ever added, this needs to
    // be one continuous lock scope.
    let raw = {
        let g = rd.lock().await;
        g.as_ref().as_raw_fd()
    };

    let (payload, fd) =
        tokio::task::spawn_blocking(move || -> std::io::Result<(Vec<u8>, Option<RawFd>)> {
            // SAFETY: `raw` is the underlying fd of the tokio `OwnedReadHalf` and
            // remains valid for this blocking task: the caller holds the
            // `Arc<Mutex<OwnedReadHalf>>` alive across the `spawn_blocking().await`
            // (the OwnedReadHalf is never moved out, never dropped, and is not
            // aliased from any other writer). We construct a temporary std handle
            // to call `recv_with_fd`, then `mem::forget` it below so the std Drop
            // does not close the fd that tokio still owns. The tokio runtime
            // retains exclusive ownership of the fd before and after this call.
            let s = unsafe { StdUnixStream::from_raw_fd(raw) };
            // NOTE: set blocking mode for `recv_with_fd` so the syscall completes
            // synchronously inside this blocking task; the fd is restored to
            // non-blocking before returning so the tokio reactor can resume
            // async I/O on the same fd next iteration. Failing to restore would
            // stall the reactor on the next async `read_exact` (Plan 2 A5
            // code-review finding).
            s.set_nonblocking(false).ok();
            let mut payload = vec![0u8; len];
            let mut fds_buf = [0i32; 1];
            let recv_res = s.recv_with_fd(&mut payload, &mut fds_buf);
            // Restore non-blocking *before* `mem::forget`, regardless of recv outcome,
            // so the tokio reactor never sees a blocking fd.
            let _ = s.set_nonblocking(true);
            std::mem::forget(s);
            let (bytes, n_fds) = recv_res?;
            if bytes != payload.len() {
                return Err(std::io::Error::other(format!(
                    "short read: expected {} got {bytes}",
                    payload.len()
                )));
            }
            let fd = if n_fds > 0 { Some(fds_buf[0]) } else { None };
            Ok((payload, fd))
        })
        .await
        .map_err(std::io::Error::other)??;

    let cmd: HostCommand = rmp_serde::from_slice(&payload).map_err(std::io::Error::other)?;
    Ok((cmd, fd))
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
