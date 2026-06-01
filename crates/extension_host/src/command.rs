//! Launches a command (bootstrap-based) extension: spawns `node <main>` with the
//! shim bin dir + command socket + piped stdin, awaits the `.ready` readiness
//! marker, and exposes `bin_dir()` for the terminal `PATH` prefix. The
//! shim/command server live in the extension (TS); this only manages the
//! process + readiness.

use crate::control::{ControlError, ControlRequest, ControlResponse, encode_response, parse_call};
use crate::host::{HostError, HostResult, LifecycleEvent, RuntimeRoot, run_lifecycle};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_ACCEPT_POLL: Duration = Duration::from_millis(20);
const READY_MARKER: &str = ".ready";

/// The oneshot the control server blocks on for the bridge's verdict.
pub type Responder = Sender<ControlResponse>;

/// Binds `sock`, accepts connections until `shutdown` is set, and turns each
/// one-shot `call` frame into `(ControlRequest, Responder)` on `req_tx`,
/// writing the bridge's verdict back as a `result`/`error` line.
pub(crate) fn serve_extension_host(
    sock: PathBuf,
    req_tx: Sender<(ControlRequest, Responder)>,
    shutdown: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("control socket bind failed: {e:?} path={sock:?}");
                return;
            }
        };
        listener.set_nonblocking(true).ok();
        loop {
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let req_tx = req_tx.clone();
                    std::thread::spawn(move || handle_control_conn(stream, req_tx));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(CONTROL_ACCEPT_POLL);
                }
                Err(_) => return,
            }
        }
    })
}

fn handle_control_conn(stream: UnixStream, req_tx: Sender<(ControlRequest, Responder)>) {
    stream.set_nonblocking(false).ok();
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut writer = stream;
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let (id, req) = match parse_call(line.trim()) {
        Ok(v) => v,
        Err(crate::control::ControlParseError::BadRequest(msg)) => {
            let resp = ControlResponse::Err(ControlError {
                code: "bad_request".into(),
                message: msg,
            });
            let _ = writer.write_all(encode_response("", &resp).as_bytes());
            return;
        }
    };
    let (resp_tx, resp_rx) = bounded::<ControlResponse>(1);
    if req_tx.send((req, resp_tx)).is_err() {
        let resp = ControlResponse::Err(ControlError {
            code: "internal".into(),
            message: "bridge gone".into(),
        });
        let _ = writer.write_all(encode_response(&id, &resp).as_bytes());
        return;
    }
    let resp = match resp_rx.recv_timeout(CONTROL_RESPONSE_TIMEOUT) {
        Ok(r) => r,
        Err(_) => ControlResponse::Err(ControlError {
            code: "internal".into(),
            message: "timeout".into(),
        }),
    };
    let _ = writer.write_all(encode_response(&id, &resp).as_bytes());
}

/// How to launch a command (bootstrap) extension.
#[derive(Clone)]
pub struct CommandExtensionConfig {
    /// Extension name (also the `EXTENSION_NAME` env + runtime-root key).
    pub name: String,
    /// Extension directory (the child's cwd).
    pub dir: PathBuf,
    /// Entry script, launched as `node <main>` (e.g. `bootstrap.ts`).
    pub main: OsString,
}

/// A running command extension. Owns the runtime root, the piped stdin (the
/// SDK's parent-death channel), and the lifecycle thread; kills the child on drop.
pub struct CommandExtension {
    bin_dir: PathBuf,
    events: Receiver<LifecycleEvent>,
    _runtime: RuntimeRoot,
    _stdin: ChildStdin,
    child: Arc<std::sync::Mutex<Option<std::process::Child>>>,
    lifecycle_shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    control_requests: Receiver<(ControlRequest, Responder)>,
    control_sock_path: PathBuf,
    handlers_sock_path: PathBuf,
    asset_sock_path: PathBuf,
    control_shutdown: Arc<AtomicBool>,
    control_thread: Option<std::thread::JoinHandle<()>>,
}

impl CommandExtension {
    /// Spawns the command extension with the default readiness timeout.
    pub fn spawn(cfg: CommandExtensionConfig) -> HostResult<Self> {
        Self::spawn_with_timeout(cfg, DEFAULT_READY_TIMEOUT)
    }

    /// Spawns with an explicit readiness timeout.
    pub fn spawn_with_timeout(
        cfg: CommandExtensionConfig,
        ready_timeout: Duration,
    ) -> HostResult<Self> {
        let runtime = RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), &cfg.name)
            .map_err(HostError::Runtime)?;
        let bin_dir = runtime.bin_dir().to_path_buf();
        let command_sock = runtime.socket_path(&cfg.name);
        let handlers_sock = runtime.socket_path(&format!("{}.handlers", cfg.name));
        let asset_sock = runtime.socket_path(&format!("{}.assets", cfg.name));
        let control_sock = runtime.socket_path(&format!("{}.control", cfg.name));

        let mut child = Command::new("node")
            .arg(&cfg.main)
            .current_dir(&cfg.dir)
            .env("OZMUX_BIN_DIR", &bin_dir)
            .env("OZMUX_SOCK_PATH", &command_sock)
            .env("EXTENSION_NAME", &cfg.name)
            .env("OZMUX_HANDLERS_SOCK_PATH", &handlers_sock)
            .env("OZMUX_ASSET_SOCK_PATH", &asset_sock)
            .env("OZMUX_CONTROL_SOCK_PATH", &control_sock)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(HostError::Spawn)?;
        let stdin = child.stdin.take().expect("piped stdin");

        let child = Arc::new(std::sync::Mutex::new(Some(child)));
        let lifecycle_shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = bounded::<LifecycleEvent>(8);

        let thread = std::thread::spawn({
            let child = Arc::clone(&child);
            let shutdown = Arc::clone(&lifecycle_shutdown);
            let bin_dir = bin_dir.clone();
            move || {
                run_lifecycle(
                    ready_timeout,
                    move || bin_dir.join(READY_MARKER).exists(),
                    || {},
                    child,
                    shutdown,
                    tx,
                );
            }
        });

        let (control_tx, control_rx) = bounded::<(ControlRequest, Responder)>(16);
        let control_shutdown = Arc::new(AtomicBool::new(false));
        let control_thread =
            serve_extension_host(control_sock.clone(), control_tx, control_shutdown.clone());

        Ok(Self {
            bin_dir,
            events: rx,
            _runtime: runtime,
            _stdin: stdin,
            child,
            lifecycle_shutdown,
            thread: Some(thread),
            control_requests: control_rx,
            control_sock_path: control_sock,
            handlers_sock_path: handlers_sock,
            asset_sock_path: asset_sock,
            control_shutdown,
            control_thread: Some(control_thread),
        })
    }

    /// The directory holding this extension's command shims (for the PATH prefix).
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    /// The channel of inbound control requests (drained by the ECS plugin).
    pub const fn control_requests(&self) -> &Receiver<(ControlRequest, Responder)> {
        &self.control_requests
    }

    /// The control socket path passed to the extension as `OZMUX_CONTROL_SOCK_PATH`.
    pub fn control_sock_path(&self) -> &Path {
        &self.control_sock_path
    }

    /// The handlers socket path the SDK binds (`OZMUX_HANDLERS_SOCK_PATH`).
    pub fn handlers_sock_path(&self) -> &Path {
        &self.handlers_sock_path
    }

    /// The asset socket path the SDK serves files on (`OZMUX_ASSET_SOCK_PATH`).
    pub fn asset_sock_path(&self) -> &Path {
        &self.asset_sock_path
    }

    /// The lifecycle event stream.
    pub const fn events(&self) -> &Receiver<LifecycleEvent> {
        &self.events
    }

    /// Blocks until `Ready`, or returns `NotReady` on `SpawnFailed`/timeout.
    pub fn wait_ready(&self, timeout: Duration) -> HostResult {
        match self.events.recv_timeout(timeout) {
            Ok(LifecycleEvent::Ready) => Ok(()),
            Ok(LifecycleEvent::SpawnFailed { .. }) | Ok(LifecycleEvent::Exited { .. }) => {
                Err(HostError::NotReady)
            }
            Err(_) => Err(HostError::NotReady),
        }
    }
}

impl Drop for CommandExtension {
    fn drop(&mut self) {
        // Signal both shutdown flags before any take()/join(): the lifecycle
        // thread may hold the child out of the mutex, in which case it must kill
        // it itself (see run_lifecycle) or the join() below hangs.
        self.control_shutdown.store(true, Ordering::SeqCst);
        self.lifecycle_shutdown.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.control_sock_path);
        if let Some(t) = self.control_thread.take() {
            let _ = t.join();
        }
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_socket_round_trips_a_split_call() {
        use crate::control::{ControlReply, ControlResponse};
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("memo.control.sock");
        let (req_tx, req_rx) = crossbeam_channel::bounded(8);
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = crate::command::serve_extension_host(sock.clone(), req_tx, shutdown.clone());

        let bridge = std::thread::spawn(move || {
            let (_req, responder) = req_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("request");
            responder
                .send(ControlResponse::Ok(ControlReply::Split {
                    new_pane_id: 42,
                    new_activity_id: 43,
                }))
                .unwrap();
        });

        // NOTE: poll until the server thread has bound the socket — bind is async
        // relative to the spawning thread; connecting before bind yields ENOENT.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if sock.exists() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "socket never appeared"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut stream = UnixStream::connect(&sock).unwrap();
        let call = r#"{"kind":"call","id":"req1","op":"split","pane":"100","params":{"side":"after","orientation":"vertical","activity":{"kind":"extension","entry":"/x","name":null,"activity_id":"aid-test"}}}"#;
        stream.write_all(format!("{call}\n").as_bytes()).unwrap();
        let mut buf = String::new();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.read_to_string(&mut buf).unwrap();
        assert!(buf.contains("\"kind\":\"result\""), "got: {buf}");
        assert!(buf.contains("\"new_pane_id\":\"42\""), "got: {buf}");

        bridge.join().unwrap();
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = UnixStream::connect(&sock);
        let _ = handle.join();
    }

    #[test]
    fn drop_stops_control_accept_loop() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("x.control.sock");
        let (tx, _rx) = crossbeam_channel::bounded(1);
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = crate::command::serve_extension_host(sock.clone(), tx, shutdown.clone());
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::os::unix::net::UnixStream::connect(&sock);
        handle.join().expect("accept loop joins");
    }

    fn memo_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/memo")
    }

    fn node_and_memo_available() -> bool {
        let node = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v node")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        node && memo_dir().join("node_modules/@ozmux/sdk").exists()
    }

    #[test]
    fn launches_memo_and_writes_shim() {
        if !node_and_memo_available() {
            eprintln!("skipping: node or memo's @ozmux/sdk link not available");
            return;
        }
        // NOTE: spawn_with_timeout (not spawn) — the lifecycle thread polls for
        // shim creation up to this budget. The default 10s starves under
        // parallel-test CPU contention (the e2e adds a third concurrent node
        // spawner); a too-low spawn budget makes the thread emit a timeout
        // event that fails wait_ready regardless of its own (larger) timeout.
        let ext = CommandExtension::spawn_with_timeout(
            CommandExtensionConfig {
                name: "memo".into(),
                dir: memo_dir(),
                main: "bootstrap.ts".into(),
            },
            Duration::from_secs(20),
        )
        .expect("spawn memo");
        ext.wait_ready(Duration::from_secs(20)).expect("memo ready");
        assert!(
            ext.bin_dir().join("@memo").exists(),
            "@memo shim must be written"
        );
    }
}
