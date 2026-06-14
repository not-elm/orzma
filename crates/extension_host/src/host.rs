//! Tokio-free host runtime: a per-handle runtime root and the host process
//! spawn + lifecycle.

use crossbeam_channel::Sender;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };

/// A per-handle runtime directory tree (`<base>/<pid>/<name>/{sock,bin}/`), removed on drop.
pub struct RuntimeRoot {
    root: PathBuf,
    sock_dir: PathBuf,
    bin_dir: PathBuf,
}

impl RuntimeRoot {
    /// Resolves a runtime root under `parent/<pid>/<name>/`, falling back to
    /// `/tmp/ozmux-ext` when the socket path would overflow the `sun_path` limit.
    pub fn resolve_in(parent: &Path, pid: u32, name: &str) -> std::io::Result<Self> {
        // NOTE: measure the LONGEST socket filename a command extension uses
        // (`<name>.handlers.sock`) so the sun_path fit check is not optimistic;
        // `socket_path` produces the shorter `<name>.sock`.
        let needed = |base: &Path| -> usize {
            base.join(pid.to_string())
                .join(name)
                .join("sock")
                .join(format!("{name}.handlers.sock"))
                .as_os_str()
                .len()
        };
        if needed(parent) <= SUN_PATH_MAX {
            return Self::new_in(parent, pid, name);
        }
        // NOTE: the shared fallback parent is created with the process umask (so
        // it is world-listable, like the legacy /tmp/ozmux); only the per-handle
        // subdir below is 0700, which is what protects the sockets.
        let fallback = Path::new("/tmp/ozmux-ext");
        std::fs::create_dir_all(fallback)?;
        if needed(fallback) <= SUN_PATH_MAX {
            return Self::new_in(fallback, pid, name);
        }
        Err(std::io::Error::other(format!(
            "'{name}' socket path exceeds {SUN_PATH_MAX} bytes"
        )))
    }

    /// The socket path for the given `name` under this root.
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

    /// The directory holding command shims.
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    fn new_in(parent: &Path, pid: u32, name: &str) -> std::io::Result<Self> {
        let root = parent.join(pid.to_string()).join(name);
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
            // NOTE: the intermediate `<parent>/<pid>` dir is created by
            // `create_dir_all` at the process umask (0755, world-listable);
            // chmod it 0700 too so handle names under it do not leak in /tmp.
            if let Some(pid_dir) = root.parent() {
                std::fs::set_permissions(pid_dir, std::fs::Permissions::from_mode(0o700))?;
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

const PROBE_INTERVAL: Duration = Duration::from_millis(20);

/// A lifecycle transition emitted by the host thread.
#[derive(Debug)]
pub enum LifecycleEvent {
    /// The host bound its socket and answered a readiness round-trip.
    Ready,
    /// The host process exited.
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
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Spawning the child process failed.
    #[error("failed to spawn host process: {0}")]
    Spawn(#[source] std::io::Error),
    /// The runtime root / socket path could not be created.
    #[error("failed to create host runtime root: {0}")]
    Runtime(#[source] std::io::Error),
    /// `wait_ready` timed out or the host failed to start.
    #[error("host did not become ready")]
    NotReady,
}

/// Convenience alias for fallible host operations.
pub type HostResult<T = ()> = Result<T, HostError>;

pub(crate) fn run_lifecycle(
    ready_timeout: Duration,
    is_ready: impl Fn() -> bool,
    on_ready: impl FnOnce(),
    child: Arc<std::sync::Mutex<Option<Child>>>,
    shutdown: Arc<AtomicBool>,
    tx: Sender<LifecycleEvent>,
) {
    // NOTE: readiness is verified via a real protocol round-trip (any well-formed
    // response), not merely a successful connect — this closes the
    // "listener-up ≠ app-ready" gap where a process could accept connections
    // before its handler is registered.
    let deadline = Instant::now() + ready_timeout;
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
                    // NOTE: if Drop set `shutdown` while we held the child out of
                    // the mutex, Drop's take() saw None and skipped the kill, so
                    // we MUST kill it here — otherwise Drop's join() hangs
                    // forever (the child never exits and we would loop putting it
                    // back). Drop signals before its take(), so this load
                    // observes it within one PROBE_INTERVAL.
                    if shutdown.load(Ordering::SeqCst) {
                        let _ = c.kill();
                        break c.wait().ok().and_then(|s| s.code());
                    }
                    *child.lock().unwrap() = Some(c);
                }
                Err(_) => break None,
            },
            None => break None,
        }
        // TODO: replace this busy-poll with an event/condvar so a long-lived
        // extension does not wake this thread every PROBE_INTERVAL (the
        // shutdown flag already makes Drop-kill exit within one interval).
        std::thread::sleep(PROBE_INTERVAL);
    };
    let _ = tx.send(LifecycleEvent::Exited { status });
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn runtime_root_creates_pid_dir_0700() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let rt = RuntimeRoot::resolve_in(parent.path(), 4242, "hello").unwrap();
        let pid_dir = rt.root().parent().unwrap();
        let mode = std::fs::metadata(pid_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "the intermediate <pid> dir must be 0700 so extension names do not leak"
        );
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
    fn runtime_roots_for_different_names_are_isolated() {
        let parent = tempfile::tempdir().unwrap();
        let a = RuntimeRoot::resolve_in(parent.path(), 99, "alpha").unwrap();
        let a_sock = a.sock_dir().to_path_buf();
        {
            let b = RuntimeRoot::resolve_in(parent.path(), 99, "beta").unwrap();
            assert_ne!(
                a.root(),
                b.root(),
                "same-PID extensions must not share a root"
            );
        } // b dropped here
        assert!(
            a_sock.exists(),
            "dropping one extension must not remove another's sockets"
        );
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
}
