//! Tokio-free extension host: a per-PID runtime root, the shared socket-path
//! endpoint, a blocking `fetch`, and (Task 3) the process spawn + lifecycle.

use crate::protocol::{ProtocolError, Request, Response, read_response, write_request};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::ffi::OsString;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// A per-PID runtime directory tree (`<base>/<pid>/{sock,bin}/`), removed on drop.
pub struct RuntimeRoot {
    root: PathBuf,
    sock_dir: PathBuf,
    bin_dir: PathBuf,
}

impl RuntimeRoot {
    /// Resolves a runtime root under `parent/<pid>/`, falling back to
    /// `/tmp/ozmux-ext` when the socket path would overflow the `sun_path` limit.
    pub fn resolve_in(parent: &Path, pid: u32, name: &str) -> std::io::Result<Self> {
        // NOTE: measure the LONGEST socket filename a command extension uses
        // (`<name>.handlers.sock`) so the sun_path fit check is not optimistic;
        // `socket_path` produces the shorter `<name>.sock`.
        let needed = |base: &Path| -> usize {
            base.join(pid.to_string())
                .join("sock")
                .join(format!("{name}.handlers.sock"))
                .as_os_str()
                .len()
        };
        if needed(parent) <= SUN_PATH_MAX {
            return Self::new_in(parent, pid);
        }
        // NOTE: the shared fallback parent is created with the process umask (so
        // it is world-listable, like the legacy /tmp/ozmux); only the per-PID
        // subdir below is 0700, which is what protects the sockets.
        let fallback = Path::new("/tmp/ozmux-ext");
        std::fs::create_dir_all(fallback)?;
        if needed(fallback) <= SUN_PATH_MAX {
            return Self::new_in(fallback, pid);
        }
        Err(std::io::Error::other(format!(
            "extension '{name}' socket path exceeds {SUN_PATH_MAX} bytes"
        )))
    }

    /// The socket path for an extension of the given `name` under this root.
    pub fn socket_path(&self, name: &str) -> PathBuf {
        self.sock_dir.join(format!("{name}.sock"))
    }

    /// The runtime root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding extension sockets.
    pub fn sock_dir(&self) -> &Path {
        &self.sock_dir
    }

    /// The directory holding extension command shims.
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    fn new_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        let root = parent.join(pid.to_string());
        let sock_dir = root.join("sock");
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&sock_dir)?;
        std::fs::create_dir_all(&bin_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&root, &sock_dir, &bin_dir] {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self {
            root,
            sock_dir,
            bin_dir,
        })
    }
}

impl Drop for RuntimeRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The resolved socket path of the (single, for this slice) live extension.
///
/// Written once by the host thread on readiness and cleared on exit; read by
/// the scheme handler on the CEF thread.
#[derive(Clone, Default)]
pub struct ExtensionEndpoints(Arc<RwLock<Option<PathBuf>>>);

impl ExtensionEndpoints {
    /// Returns the live socket path, or `None` before readiness / after exit.
    pub fn get(&self) -> Option<PathBuf> {
        self.0.read().unwrap().clone()
    }

    pub(crate) fn set(&self, path: PathBuf) {
        *self.0.write().unwrap() = Some(path);
    }

    pub(crate) fn clear(&self) {
        *self.0.write().unwrap() = None;
    }
}

/// A failure while fetching an asset from the extension.
#[derive(Debug)]
pub enum FetchError {
    /// No live endpoint yet (pre-readiness or post-exit).
    NotReady,
    /// Connect / read / write / timeout failure.
    Io(std::io::Error),
    /// The response frame was malformed.
    Protocol(ProtocolError),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::NotReady => write!(f, "extension endpoint is not ready"),
            FetchError::Io(e) => write!(f, "extension fetch I/O error: {e}"),
            FetchError::Protocol(e) => write!(f, "extension protocol error: {e}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Io(e) => Some(e),
            FetchError::Protocol(e) => Some(e),
            FetchError::NotReady => None,
        }
    }
}

/// Fetches `path` from the currently-live extension endpoint.
pub fn fetch(endpoints: &ExtensionEndpoints, path: &str) -> Result<Response, FetchError> {
    let sock = endpoints.get().ok_or(FetchError::NotReady)?;
    fetch_at(&sock, path)
}

pub(crate) fn fetch_at(sock: &Path, path: &str) -> Result<Response, FetchError> {
    let mut stream = UnixStream::connect(sock).map_err(FetchError::Io)?;
    stream
        .set_read_timeout(Some(FETCH_TIMEOUT))
        .map_err(FetchError::Io)?;
    stream
        .set_write_timeout(Some(FETCH_TIMEOUT))
        .map_err(FetchError::Io)?;
    write_request(
        &mut stream,
        &Request {
            path: path.to_string(),
        },
    )
    .map_err(map_proto)?;
    stream.shutdown(Shutdown::Write).map_err(FetchError::Io)?;
    read_response(&mut stream).map_err(map_proto)
}

fn map_proto(e: ProtocolError) -> FetchError {
    match e {
        ProtocolError::Io(io) => FetchError::Io(io),
        other => FetchError::Protocol(other),
    }
}

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_INTERVAL: Duration = Duration::from_millis(20);

/// How to launch an extension. Generic over the program for testability; use
/// [`ExtensionConfig::node`] for the real Node launch contract.
pub struct ExtensionConfig {
    /// Extension name (the `<name>` in `ozmux-ext://<name>/…`).
    pub name: String,
    /// Program to spawn (e.g. `"node"`).
    pub program: OsString,
    /// Arguments (e.g. `[main.ts]`).
    pub args: Vec<OsString>,
    /// Working directory for the child.
    pub dir: PathBuf,
}

impl ExtensionConfig {
    /// Builds a config that launches `node <main>` (the legacy launch contract).
    pub fn node(name: impl Into<String>, dir: PathBuf, main: impl Into<OsString>) -> Self {
        Self {
            name: name.into(),
            program: "node".into(),
            args: vec![main.into()],
            dir,
        }
    }
}

/// A lifecycle transition emitted by the host thread.
#[derive(Debug)]
pub enum LifecycleEvent {
    /// The extension bound its socket and answered a readiness round-trip.
    Ready,
    /// The extension process exited.
    Exited {
        /// Exit code, if known.
        status: Option<i32>,
    },
    /// The process never became ready within the timeout.
    SpawnFailed {
        /// Human-readable reason.
        error: String,
    },
}

/// A failure to start the host.
#[derive(Debug)]
pub enum HostError {
    /// Spawning the child process failed.
    Spawn(std::io::Error),
    /// The runtime root / socket path could not be created.
    Runtime(std::io::Error),
    /// `wait_ready` timed out or the extension failed to start.
    NotReady,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Spawn(e) => write!(f, "failed to spawn extension process: {e}"),
            HostError::Runtime(e) => write!(f, "failed to create extension runtime root: {e}"),
            HostError::NotReady => write!(f, "extension did not become ready"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HostError::Spawn(e) | HostError::Runtime(e) => Some(e),
            HostError::NotReady => None,
        }
    }
}

/// Convenience alias for fallible host operations.
pub type HostResult<T = ()> = Result<T, HostError>;

/// A running extension: owns the runtime root + lifecycle thread.
pub struct ExtensionHost {
    endpoints: ExtensionEndpoints,
    events: Receiver<LifecycleEvent>,
    _runtime: Arc<RuntimeRoot>,
    child: Arc<std::sync::Mutex<Option<Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ExtensionHost {
    /// Spawns the extension with the default readiness timeout (non-blocking).
    pub fn spawn(cfg: ExtensionConfig) -> HostResult<Self> {
        Self::spawn_with_timeout(cfg, DEFAULT_READY_TIMEOUT)
    }

    /// Spawns with an explicit readiness timeout.
    pub fn spawn_with_timeout(cfg: ExtensionConfig, ready_timeout: Duration) -> HostResult<Self> {
        let runtime = RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), &cfg.name)
            .map_err(HostError::Runtime)?;
        let sock = runtime.socket_path(&cfg.name);
        let child = Command::new(&cfg.program)
            .args(&cfg.args)
            .current_dir(&cfg.dir)
            .env("OZMUX_SOCK_PATH", &sock)
            .spawn()
            .map_err(HostError::Spawn)?;

        let runtime = Arc::new(runtime);
        let child = Arc::new(std::sync::Mutex::new(Some(child)));
        let endpoints = ExtensionEndpoints::default();
        let (tx, rx) = bounded::<LifecycleEvent>(8);

        let endpoints_for_clear = endpoints.clone();
        let thread = std::thread::spawn({
            let endpoints = endpoints.clone();
            let child = Arc::clone(&child);
            let sock = sock.clone();
            move || {
                let ready_sock = sock.clone();
                run_lifecycle(
                    ready_timeout,
                    move || fetch_at(&ready_sock, "index.html").is_ok(),
                    move || endpoints.set(sock),
                    child,
                    tx,
                );
                endpoints_for_clear.clear();
            }
        });

        Ok(Self {
            endpoints,
            events: rx,
            _runtime: runtime,
            child,
            thread: Some(thread),
        })
    }

    /// A clone of the shared endpoint handle (for the scheme handler).
    pub fn endpoints(&self) -> ExtensionEndpoints {
        self.endpoints.clone()
    }

    /// The lifecycle event stream (the stable seam the ECS plugin will drain).
    pub fn events(&self) -> &Receiver<LifecycleEvent> {
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

impl Drop for ExtensionHost {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn run_lifecycle(
    ready_timeout: Duration,
    is_ready: impl Fn() -> bool,
    on_ready: impl FnOnce(),
    child: Arc<std::sync::Mutex<Option<Child>>>,
    tx: Sender<LifecycleEvent>,
) {
    // NOTE: readiness is verified via a real protocol round-trip (any well-formed
    // response), not merely a successful connect — this closes the
    // "listener-up ≠ app-ready" gap where a process could accept connections
    // before its handler is registered.
    let deadline = Instant::now() + ready_timeout;
    // TODO: each fetch_at attempt uses the fixed FETCH_TIMEOUT (5s) for its
    // read/write, so a ready_timeout shorter than 5s is only honored between
    // attempts, not during one hung attempt. Parametrize fetch_at's timeout if
    // an extension can bind the socket but stall on the first request.
    let ready = loop {
        if is_ready() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(PROBE_INTERVAL);
    };

    if !ready {
        // NOTE: we take the child out of the mutex before killing so Drop's
        // lock().take() returns None and does not deadlock waiting for a lock
        // held across a blocking wait().
        let taken = child.lock().unwrap().take();
        if let Some(mut c) = taken {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = tx.send(LifecycleEvent::SpawnFailed {
            error: "readiness timeout".into(),
        });
        return;
    }

    on_ready();
    let _ = tx.send(LifecycleEvent::Ready);

    // NOTE: poll try_wait() rather than blocking in wait() so that Drop can
    // acquire the mutex, kill the child, and have the poll loop detect exit.
    // A blocking wait() after take() would prevent Drop from killing the child
    // and cause t.join() to hang until the extension exits on its own.
    let status = loop {
        let taken = { child.lock().unwrap().take() };
        match taken {
            Some(mut c) => match c.try_wait() {
                Ok(Some(s)) => break s.code(),
                Ok(None) => {
                    *child.lock().unwrap() = Some(c);
                }
                Err(_) => break None,
            },
            None => break None,
        }
        // TODO: replace this busy-poll with a shutdown signal (AtomicBool /
        // channel set by Drop before kill) so Drop-kill exits in O(1) and a
        // long-lived extension does not wake this thread every PROBE_INTERVAL.
        std::thread::sleep(PROBE_INTERVAL);
    };
    let _ = tx.send(LifecycleEvent::Exited { status });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Response, read_request, write_response};
    use std::os::unix::net::UnixListener;

    fn serve_once(sock: std::path::PathBuf, resp: Response) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_request(&mut stream);
                let _ = write_response(&mut stream, &resp);
            }
        })
    }

    #[test]
    fn fetch_returns_served_response() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("e.sock");
        let h = serve_once(
            sock.clone(),
            Response {
                status: 200,
                content_type: "text/html".into(),
                body: b"<h1>hi</h1>".to_vec(),
            },
        );
        let got = fetch_at(&sock, "index.html").unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, b"<h1>hi</h1>");
        h.join().unwrap();
    }

    #[test]
    fn fetch_not_ready_when_endpoint_unset() {
        let endpoints = ExtensionEndpoints::default();
        assert!(matches!(
            fetch(&endpoints, "index.html"),
            Err(FetchError::NotReady)
        ));
    }

    #[test]
    fn fetch_io_error_when_socket_absent() {
        let missing = std::path::Path::new("/tmp/ozmux-does-not-exist.sock");
        assert!(matches!(fetch_at(missing, "x"), Err(FetchError::Io(_))));
    }

    #[test]
    fn runtime_root_creates_sock_dir_0700_and_drops() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let path = {
            let rt = RuntimeRoot::resolve_in(parent.path(), 4242, "hello").unwrap();
            let mode = std::fs::metadata(rt.sock_dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
            assert_eq!(rt.socket_path("hello").parent().unwrap(), rt.sock_dir());
            rt.root().to_path_buf()
        };
        assert!(!path.exists(), "Drop must remove the tree");
    }

    #[test]
    fn runtime_root_creates_bin_dir_0700() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let rt = RuntimeRoot::resolve_in(parent.path(), 4243, "memo").unwrap();
        let mode = std::fs::metadata(rt.bin_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn runtime_root_falls_back_to_tmp_when_too_long() {
        let deep = std::iter::repeat_n("a", 120).collect::<Vec<_>>().join("/");
        let outer = tempfile::tempdir().unwrap();
        let parent = outer.path().join(deep);
        std::fs::create_dir_all(&parent).unwrap();
        let rt = RuntimeRoot::resolve_in(&parent, 7, "hello").unwrap();
        assert!(
            rt.root().starts_with("/tmp"),
            "expected /tmp fallback, got {:?}",
            rt.root()
        );
    }

    #[test]
    fn spawn_failed_when_program_never_binds() {
        let cfg = ExtensionConfig {
            name: "never-binds".into(),
            program: "sleep".into(),
            args: vec!["5".into()],
            dir: std::env::temp_dir(),
        };
        let host = ExtensionHost::spawn_with_timeout(cfg, Duration::from_millis(300)).unwrap();
        match host.events().recv_timeout(Duration::from_secs(2)) {
            Ok(LifecycleEvent::SpawnFailed { .. }) => {}
            other => panic!("expected SpawnFailed, got {other:?}"),
        }
    }

    #[test]
    fn ready_when_program_binds_socket() {
        if std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v nc")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: nc not available");
            return;
        }
        let cfg = ExtensionConfig {
            name: "nc-ready".into(),
            program: "sh".into(),
            args: vec!["-c".into(),
                "printf '\\000\\310\\000\\000\\000\\011text/html\\000\\000\\000\\002ok' | nc -lU \"$OZMUX_SOCK_PATH\"".into()],
            dir: std::env::temp_dir(),
        };
        let host = ExtensionHost::spawn(cfg).unwrap();
        assert!(host.wait_ready(Duration::from_secs(3)).is_ok());
    }
}
