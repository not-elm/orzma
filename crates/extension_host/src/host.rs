//! Tokio-free extension host: a per-PID runtime root, the shared socket-path
//! endpoint, a blocking `fetch`, and (Task 3) the process spawn + lifecycle.

use crate::protocol::{ProtocolError, Request, Response, read_response, write_request};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// A per-PID runtime directory tree (`<base>/<pid>/sock/`), removed on drop.
pub struct RuntimeRoot {
    root: PathBuf,
    sock_dir: PathBuf,
}

impl RuntimeRoot {
    /// Resolves a runtime root under `parent/<pid>/`, falling back to
    /// `/tmp/ozmux-ext` when the socket path would overflow the `sun_path` limit.
    pub fn resolve_in(parent: &Path, pid: u32, name: &str) -> std::io::Result<Self> {
        let needed = |base: &Path| -> usize {
            base.join(pid.to_string())
                .join("sock")
                .join(format!("{name}.sock"))
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

    fn new_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        let root = parent.join(pid.to_string());
        let sock_dir = root.join("sock");
        std::fs::create_dir_all(&sock_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&root, &sock_dir] {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self { root, sock_dir })
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
