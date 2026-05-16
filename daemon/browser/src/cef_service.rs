//! Daemon-side supervisor for the `cef_host` child process.
//!
//! Spawns the child, accepts its UDS connection, exchanges `Hello` / `Ready`,
//! then runs a bidirectional pump that forwards `HostCommand`s to the child and
//! surfaces `HostEvent`s back to callers via `CefHostHandles`.

use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{CefCookieDto, HostCommand, HostEvent};
use std::os::fd::OwnedFd;
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
    /// Multi-producer mpsc into the pump task. Cloneable; every clone may
    /// independently push `HostCommand`s. The pump forwards each command to
    /// the `cef_host` child in arrival order.
    pub commands: mpsc::Sender<HostCommand>,
    /// Single-consumer mpsc out of the pump task. Not `Clone`; once consumed
    /// by `recv()` an event is gone. Daemon-side fan-out (per-activity event
    /// routing) happens by draining this receiver in one place and
    /// re-broadcasting downstream.
    pub events: mpsc::Receiver<HostEvent>,
    /// The spawned `cef_host` child. Dropping this handle does **not** kill
    /// the child — callers that need shutdown must call
    /// [`tokio::process::Child::kill`] (or wait on `wait()`) explicitly.
    pub child: Child,
}

impl CefHostHandles {
    /// Sends a `HostCommand` without ancillary data. Returns `Err` if the
    /// internal mpsc to the pump task has been closed (cef_host died or the
    /// supervisor was dropped).
    pub async fn send_command(&self, cmd: HostCommand) -> std::io::Result<()> {
        self.commands
            .send(cmd)
            .await
            .map_err(|_| std::io::Error::other("control channel closed"))
    }

    /// Sends a `HostCommand::BrowserCreate` carrying the shm fd as ancillary
    /// data via `sendmsg_with_fds`.
    ///
    /// **Stub:** currently returns
    /// `Err(io::Error::other("request_browser_create wired in Task A5"))`.
    /// Callers must propagate this error (do not unwrap). Task A5 replaces the
    /// body with the real `sendmsg_with_fds` implementation; the signature is
    /// stable.
    pub async fn request_browser_create(
        &self,
        _aid: ActivityId,
        _initial_url: String,
        _epoch: u32,
        _cookies: Vec<CefCookieDto>,
        _shm_fd: OwnedFd,
    ) -> std::io::Result<()> {
        // TODO: Task A5 — replace the stub with a real sendmsg_with_fds path
        // that serializes the BrowserCreate command and attaches `_shm_fd` as
        // ancillary data on the same syscall (parent spec §5).
        Err(std::io::Error::other(
            "request_browser_create wired in Task A5",
        ))
    }
}

impl CefHostSupervisor {
    /// Creates a supervisor that listens at `socket_path` and (via
    /// `spawn_and_handshake`) spawns a `cef_host` child pointing at it.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Listens on the UDS, spawns the `cef_host` binary, accepts its inbound
    /// connection, exchanges `Hello` / `Ready`, then starts the bidirectional
    /// pump task. Per-BrowserCreate shm fds are transferred in Task A5.
    pub async fn spawn_and_handshake(&self) -> std::io::Result<CefHostHandles> {
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

/// Constructs a `CefHostHandles` that does not launch a real cef_host. Used by
/// daemon unit tests that build `AppState` but never exercise the cef path.
/// Must be called from within a Tokio runtime context.
#[doc(hidden)]
pub fn stub_for_tests() -> CefHostHandles {
    let (tx, _rx) = mpsc::channel::<HostCommand>(8);
    let (_ev_tx, ev_rx) = mpsc::channel::<HostEvent>(8);
    let child = Command::new("true")
        .spawn()
        .expect("`true` should always spawn");
    CefHostHandles {
        commands: tx,
        events: ev_rx,
        child,
    }
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
