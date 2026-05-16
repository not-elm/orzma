//! Daemon-side supervisor for the `cef_host` child process.
//!
//! PoC scope: spawn the child, accept the UDS connection, exchange
//! `Hello` / `Ready`, transfer one shm fd via SCM_RIGHTS, then run a
//! bidirectional pump that forwards `HostCommand`s to the child and surfaces
//! `HostEvent`s back to callers.

use ozmux_browser_cef_protocol::wire::{HostCommand, HostEvent};
use sendfd::SendWithFd;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Owns the daemon side of the daemon ↔ cef_host channel.
pub struct CefHostSupervisor {
    socket_path: PathBuf,
}

/// Sender + receiver pair returned after a successful handshake.
pub struct CefHostHandles {
    pub commands: mpsc::Sender<HostCommand>,
    pub events: mpsc::Receiver<HostEvent>,
    /// The spawned cef_host child. Kept so callers can wait on / kill it.
    pub child: Child,
}

impl CefHostSupervisor {
    /// Creates a supervisor that listens at `socket_path` and (via
    /// `spawn_and_handshake`) spawns a `cef_host` child pointing at it.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Listens on the UDS, spawns the `cef_host` binary, accepts its inbound
    /// connection, exchanges `Hello` / `Ready`, transfers `shm_fd` via
    /// SCM_RIGHTS, then starts the bidirectional pump task.
    pub async fn spawn_and_handshake(&self, shm_fd: OwnedFd) -> std::io::Result<CefHostHandles> {
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }
        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(socket = %self.socket_path.display(), "listening for cef_host");

        let cef_host_bin = std::env::var("OZMUX_CEF_HOST_BIN")
            .unwrap_or_else(|_| "./target/debug/cef_host".into());
        let child = Command::new(&cef_host_bin)
            .env("OZMUX_CEF_HOST_SOCKET", &self.socket_path)
            .spawn()?;
        tracing::info!(pid = child.id(), bin = %cef_host_bin, "spawned cef_host");

        let (stream, _addr) = listener.accept().await?;
        let (mut rd, mut wr) = stream.into_split();

        let hello: HostEvent = recv_msg(&mut rd).await?;
        tracing::info!(?hello, "received Hello from cef_host");

        let ready = HostCommand::Ready {
            runtime_root: "/tmp/ozmux".into(),
        };
        send_msg(&mut wr, &ready).await?;
        tracing::info!("sent Ready");

        send_shm_fd_sync(&wr, shm_fd.as_raw_fd())?;
        tracing::info!("sent shm fd via SCM_RIGHTS");
        // NOTE: cef_host duplicates the fd via recvmsg; drop our copy now so
        // the region's refcount tracks only the child's lifetime.
        drop(shm_fd);

        let (cmd_tx, cmd_rx) = mpsc::channel::<HostCommand>(64);
        let (ev_tx, ev_rx) = mpsc::channel::<HostEvent>(64);
        tokio::spawn(pump(rd, wr, cmd_rx, ev_tx));

        Ok(CefHostHandles {
            commands: cmd_tx,
            events: ev_rx,
            child,
        })
    }
}

fn send_shm_fd_sync(wr: &OwnedWriteHalf, shm_fd: i32) -> std::io::Result<()> {
    // NOTE: SCM_RIGHTS requires sendmsg on the underlying socket; tokio's
    // OwnedWriteHalf does not expose this directly. We borrow the raw fd to
    // construct a transient std UnixStream, send one byte of payload + the
    // ancillary fd, then forget the std handle so it does not close the fd
    // the tokio half still owns.
    let raw = wr.as_ref().as_raw_fd();
    // SAFETY: `raw` is owned by `wr` for the duration of this borrow; we
    // immediately forget the std handle below to prevent double-close.
    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw) };
    let res = std_stream.send_with_fd(b"s", &[shm_fd]);
    std::mem::forget(std_stream);
    res.map(|_| ())
}

async fn pump(
    mut rd: OwnedReadHalf,
    mut wr: OwnedWriteHalf,
    mut cmd_rx: mpsc::Receiver<HostCommand>,
    ev_tx: mpsc::Sender<HostEvent>,
) {
    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                if let Err(e) = send_msg(&mut wr, &cmd).await {
                    tracing::warn!(error = %e, "send to cef_host failed; pump exiting");
                    break;
                }
            }
            ev = recv_msg::<HostEvent>(&mut rd) => {
                match ev {
                    Ok(e) => {
                        if ev_tx.send(e).await.is_err() {
                            tracing::debug!("event consumer dropped; pump exiting");
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "recv from cef_host failed; pump exiting");
                        break;
                    }
                }
            }
            else => break,
        }
    }
}

async fn send_msg<T: serde::Serialize>(wr: &mut OwnedWriteHalf, msg: &T) -> std::io::Result<()> {
    let payload = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::other("control frame too large"))?;
    wr.write_all(&len.to_be_bytes()).await?;
    wr.write_all(&payload).await?;
    wr.flush().await?;
    Ok(())
}

async fn recv_msg<T: serde::de::DeserializeOwned>(rd: &mut OwnedReadHalf) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    rd.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    rd.read_exact(&mut payload).await?;
    rmp_serde::from_slice(&payload).map_err(std::io::Error::other)
}
