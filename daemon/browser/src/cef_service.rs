//! Daemon-side supervisor for the `cef_host` child process.
//!
//! Spawns the child, accepts its UDS connection, exchanges `Hello` / `Ready`,
//! then runs a bidirectional pump that forwards `HostCommand`s to the child and
//! surfaces `HostEvent`s back to callers via `CefHostHandles`.
//!
//! Per-BrowserCreate shm fds travel via SCM_RIGHTS on the same sendmsg as the
//! serialised `BrowserCreate` body (Task A5). A dedicated `ScmSend` mpsc feeds
//! a `spawn_blocking` arm in the pump that issues the `write_all(len)` +
//! `send_with_fd(body, &[fd])` pair.

use crate::cef_registry::{BrowserCefRegistry, NavState};
use crate::frame_ring::FrameEnvelope;
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{BrowserProfileWire, CefCookieDto, HostCommand, HostEvent};
use sendfd::SendWithFd;
use std::io::Write as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc};

/// A framed payload that must be delivered with an ancillary fd via SCM_RIGHTS.
struct ScmSend {
    payload: Vec<u8>,
    fd: OwnedFd,
}

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
    /// Single-consumer mpsc out of the pump task. Wrapped in `StdMutex<Option>`
    /// so [`events_take`](Self::events_take) can move it out exactly once
    /// through a shared `Arc<CefHostHandles>` at daemon startup. Tests that
    /// need raw access take it before the pump is spawned.
    pub events: StdMutex<Option<mpsc::Receiver<HostEvent>>>,
    /// The spawned `cef_host` child. Wrapped in `StdMutex<Option>` so
    /// [`take_child`](Self::take_child) can move it out exactly once into the
    /// crash-watcher task spawned at daemon startup.
    child: StdMutex<Option<Child>>,
    /// Set to `true` (Release) by the crash-watcher task when `cef_host` exits
    /// unexpectedly. Read with Acquire ordering via [`is_dead`](Self::is_dead).
    is_dead: Arc<AtomicBool>,
    scm_tx: mpsc::Sender<ScmSend>,
}

impl CefHostHandles {
    /// Moves the event receiver out of this handle.
    ///
    /// Returns `Some` on the first call; subsequent calls return `None`. The
    /// event pump spawned at daemon startup calls this exactly once through an
    /// `Arc<CefHostHandles>`. Integration tests that need raw access to events
    /// may call this before the pump starts.
    pub fn events_take(&self) -> Option<mpsc::Receiver<HostEvent>> {
        self.events
            .lock()
            .expect("CefHostHandles.events poisoned")
            .take()
    }

    /// Moves the `Child` handle out of this struct exactly once.
    ///
    /// Returns `Some` on the first call; subsequent calls return `None`.
    /// The crash-watcher task spawned at daemon startup calls this to obtain
    /// the child so it can `await child.wait()` without holding the mutex
    /// across an async boundary.
    pub fn take_child(&self) -> Option<Child> {
        self.child
            .lock()
            .expect("CefHostHandles.child poisoned")
            .take()
    }

    /// Returns `true` if the `cef_host` child has exited unexpectedly.
    ///
    /// Set by the crash-watcher task via [`is_dead_handle`](Self::is_dead_handle).
    /// Uses Acquire ordering.
    pub fn is_dead(&self) -> bool {
        self.is_dead.load(Ordering::Acquire)
    }

    /// Returns a clone of the `Arc<AtomicBool>` that the crash-watcher task
    /// uses to signal unexpected exit. Callers store `true` with Release ordering.
    pub fn is_dead_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_dead)
    }

    /// Sends a `HostCommand` without ancillary data. Returns `Err` if the
    /// internal mpsc to the pump task has been closed (cef_host died or the
    /// supervisor was dropped).
    pub async fn send_command(&self, cmd: HostCommand) -> std::io::Result<()> {
        self.commands
            .send(cmd)
            .await
            .map_err(|_| std::io::Error::other("control channel closed"))
    }

    /// Sends a `HostCommand::BrowserCreate` with `shm_fd` as ancillary data
    /// via `sendmsg_with_fds`. The fd arrives at cef_host paired with the
    /// same recvmsg that delivers the serialised body. `profile` is carried
    /// inside the command and selects the embedded browser's storage profile.
    pub async fn request_browser_create(
        &self,
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        cookies: Vec<CefCookieDto>,
        profile: BrowserProfileWire,
        shm_fd: OwnedFd,
    ) -> std::io::Result<()> {
        let cmd = HostCommand::BrowserCreate {
            aid,
            initial_url,
            epoch,
            cookies,
            profile,
        };
        let payload = rmp_serde::to_vec_named(&cmd).map_err(std::io::Error::other)?;
        self.scm_tx
            .send(ScmSend {
                payload,
                fd: shm_fd,
            })
            .await
            .map_err(|_| std::io::Error::other("scm channel closed"))
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
    /// pump task. Per-BrowserCreate shm fds are transferred via SCM_RIGHTS on
    /// the same sendmsg as the serialised `BrowserCreate` body.
    pub async fn spawn_and_handshake(&self) -> std::io::Result<CefHostHandles> {
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }
        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(socket = %self.socket_path.display(), "listening for cef_host");

        let cef_host_bin = std::env::var("OZMUX_CEF_HOST_BIN")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| default_cef_host_bin());
        let child = Command::new(&cef_host_bin)
            .env("OZMUX_CEF_HOST_SOCKET", &self.socket_path)
            .spawn()?;
        tracing::info!(pid = child.id(), bin = %cef_host_bin.display(), "spawned cef_host");

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
        let (scm_tx, scm_rx) = mpsc::channel::<ScmSend>(8);

        let rd = Arc::new(Mutex::new(rd));
        let wr = Arc::new(Mutex::new(wr));
        tokio::spawn(pump(rd, wr, cmd_rx, scm_rx, ev_tx));

        Ok(CefHostHandles {
            commands: cmd_tx,
            events: StdMutex::new(Some(ev_rx)),
            child: StdMutex::new(Some(child)),
            is_dead: Arc::new(AtomicBool::new(false)),
            scm_tx,
        })
    }
}

/// Default path to the `cef_host` executable, resolved relative to the
/// daemon binary's own directory so that a working-directory change (cargo
/// test, daemon spawned via Tauri, etc.) does not break the lookup. On
/// macOS, multi-process CEF requires the executable to live inside a `.app`
/// bundle with a `CFBundleIdentifier` (Mach port rendezvous is namespaced by
/// bundle ID), so we point at the bundled executable assembled by
/// `xtask bundle-cef-host`. On other platforms the bare binary sibling is
/// used. Override with `OZMUX_CEF_HOST_BIN`.
fn default_cef_host_bin() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    #[cfg(target_os = "macos")]
    {
        exe_dir
            .join("cef_host.app")
            .join("Contents/MacOS/cef_host")
    }
    #[cfg(not(target_os = "macos"))]
    {
        exe_dir.join("cef_host")
    }
}

/// Constructs a `CefHostHandles` that does not launch a real cef_host. Used by
/// daemon unit tests that build `AppState` but never exercise the cef path.
/// Must be called from within a Tokio runtime context.
#[doc(hidden)]
pub fn stub_for_tests() -> CefHostHandles {
    let (tx, _rx) = mpsc::channel::<HostCommand>(8);
    let (_ev_tx, ev_rx) = mpsc::channel::<HostEvent>(8);
    let (scm_tx, _scm_rx) = mpsc::channel::<ScmSend>(8);
    let child = Command::new("true")
        .spawn()
        .expect("`true` should always spawn");
    CefHostHandles {
        commands: tx,
        events: StdMutex::new(Some(ev_rx)),
        child: StdMutex::new(Some(child)),
        is_dead: Arc::new(AtomicBool::new(false)),
        scm_tx,
    }
}

async fn pump(
    rd: Arc<Mutex<OwnedReadHalf>>,
    wr: Arc<Mutex<OwnedWriteHalf>>,
    mut cmd_rx: mpsc::Receiver<HostCommand>,
    mut scm_rx: mpsc::Receiver<ScmSend>,
    ev_tx: mpsc::Sender<HostEvent>,
) {
    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                if let Err(e) = send_msg_arc(&wr, &cmd).await {
                    tracing::warn!(error = %e, "send to cef_host failed; pump exiting");
                    break;
                }
            }
            Some(ScmSend { payload, fd }) = scm_rx.recv() => {
                let raw_w = {
                    let g = wr.lock().await;
                    g.as_ref().as_raw_fd()
                };
                let fd_raw = fd.as_raw_fd();
                let result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                    // SAFETY: `raw_w` is the underlying fd of the tokio
                    // `OwnedWriteHalf`. The Arc<Mutex<OwnedWriteHalf>> owning it is
                    // held across this `spawn_blocking().await` by the pump task,
                    // so the fd cannot be closed while this blocking task runs.
                    // We `mem::forget` the std handle below so its Drop does not
                    // close the fd that the tokio runtime still owns.
                    let s = unsafe { StdUnixStream::from_raw_fd(raw_w) };
                    let len_be = u32::try_from(payload.len())
                        .map_err(|_| std::io::Error::other("payload too large"))?
                        .to_be_bytes();
                    let res: std::io::Result<()> = (|| {
                        (&s).write_all(&len_be)?;
                        s.send_with_fd(&payload, &[fd_raw])?;
                        Ok(())
                    })();
                    std::mem::forget(s);
                    // NOTE: `fd` (OwnedFd) drops at closure end, which closes our
                    // copy of the shm fd. cef_host has already received its own
                    // dup via recvmsg+SCM_RIGHTS so the region survives via the
                    // child's copy.
                    res
                })
                .await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "scm send failed; pump exiting");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = ?e, "scm send join failed");
                    }
                }
            }
            ev = recv_msg_arc::<HostEvent>(&rd) => {
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

async fn send_msg_arc<T: serde::Serialize>(
    wr: &Arc<Mutex<OwnedWriteHalf>>,
    msg: &T,
) -> std::io::Result<()> {
    let payload = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::other("control frame too large"))?;
    let mut g = wr.lock().await;
    g.write_all(&len.to_be_bytes()).await?;
    g.write_all(&payload).await?;
    g.flush().await?;
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

async fn recv_msg_arc<T: serde::de::DeserializeOwned>(
    rd: &Arc<Mutex<OwnedReadHalf>>,
) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    let mut g = rd.lock().await;
    g.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    g.read_exact(&mut payload).await?;
    rmp_serde::from_slice(&payload).map_err(std::io::Error::other)
}

/// Spawns a task that drains `HostEvent`s from the cef_host channel and routes
/// them to per-activity sinks on `BrowserCefRegistry`.
///
/// Takes the event receiver out of `handles` via [`CefHostHandles::events_take`]
/// exactly once. Returns the `JoinHandle` so the caller (daemon_bootstrap) can
/// hold it for the process lifetime; dropping the handle cancels the task.
///
/// # Panics
///
/// Panics if `handles.events_take()` returns `None` (i.e. the receiver was
/// already consumed by a previous call or by a test).
pub fn spawn_event_pump(
    handles: Arc<CefHostHandles>,
    registry: Arc<BrowserCefRegistry>,
) -> tokio::task::JoinHandle<()> {
    let events = handles.events_take().expect(
        "spawn_event_pump: events receiver already consumed; call this function exactly once",
    );
    tokio::spawn(event_pump_loop(events, registry))
}

async fn event_pump_loop(mut events: mpsc::Receiver<HostEvent>, registry: Arc<BrowserCefRegistry>) {
    loop {
        let Some(ev) = events.recv().await else {
            tracing::debug!("cef_host event channel closed; pump exiting");
            break;
        };
        match ev {
            HostEvent::NavStateChanged {
                aid,
                url,
                title,
                can_back,
                can_forward,
            } => {
                let next = NavState {
                    url,
                    title,
                    can_back,
                    can_forward,
                };
                if let Err(e) = registry.nav_publish(&aid, next) {
                    tracing::debug!(error = %e, "nav_publish: aid not in registry");
                }
            }
            HostEvent::TitleChanged { aid, title } => {
                if let Some(mut current) = registry.nav_current(&aid) {
                    current.title = title;
                    if let Err(e) = registry.nav_publish(&aid, current) {
                        tracing::debug!(error = %e, "nav_publish (TitleChanged): aid not in registry");
                    }
                }
            }
            HostEvent::CursorChanged { aid, cursor } => {
                if let Err(e) = registry.cursor_publish(&aid, cursor) {
                    tracing::debug!(error = %e, "cursor_publish: aid not in registry");
                }
            }
            // On each frame cef_host writes to shm and emits a FrameDescriptor;
            // the pump reads the named slot and publishes it into the ring so
            // WS subscribers receive a Screencast.
            HostEvent::FrameDescriptor {
                aid,
                slot_idx,
                is_popup,
                ..
            } => {
                let Some((ring, reader)) = registry.ring_and_reader(&aid) else {
                    tracing::debug!(aid = %aid.0, "FrameDescriptor: aid not in registry");
                    continue;
                };
                let snapshot = if is_popup {
                    reader.reader().read_popup()
                } else {
                    reader.reader().read_stable(slot_idx as usize)
                };
                match snapshot {
                    Some(s) => ring.push(Arc::new(FrameEnvelope {
                        session_id: ring.session_id(),
                        epoch: ring.epoch(),
                        frame_seq: s.frame_seq,
                        captured_at_us: s.captured_at_us,
                        width: s.width,
                        height: s.height,
                        is_keyframe: s.is_keyframe,
                        damage_rects: s.damage_rects,
                        is_popup: s.is_popup,
                        bgra: s.payload,
                    })),
                    None => {
                        tracing::debug!(aid = %aid.0, slot_idx, "FrameDescriptor: shm slot unstable")
                    }
                }
            }
            // NOTE: BrowserReady is consumed by integration tests via direct
            // receiver access; Hello is handled during the handshake.
            HostEvent::BrowserReady { .. } | HostEvent::Hello { .. } => {}
            // NOTE: remaining variants are informational or deferred to later tasks.
            HostEvent::SelectionChanged { .. }
            | HostEvent::PageError { .. }
            | HostEvent::RenderProcessTerminated { .. }
            | HostEvent::LogLine { .. }
            | HostEvent::Crashed { .. } => {
                tracing::debug!(?ev, "event pump: unhandled HostEvent variant");
            }
        }
    }
}
