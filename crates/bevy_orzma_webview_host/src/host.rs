//! Tokio-free host runtime: a per-handle runtime root used to mint the
//! user-private socket directory tree for the webview control plane.

use crate::private_dir::restrict_to_current_user;
use std::path::{Path, PathBuf};

const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };

/// Error returned when resolving a [`RuntimeRoot`].
#[derive(Debug, thiserror::Error)]
pub enum RuntimeRootError {
    /// The longest socket path under the chosen root would overflow `sun_path`.
    #[error("'{name}' socket path exceeds {limit} bytes")]
    SocketPathTooLong {
        /// Webview handle name whose socket path overflowed.
        name: String,
        /// The `sun_path` byte limit that was exceeded.
        limit: usize,
    },

    /// Creating or permissioning the runtime directories failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A per-handle runtime directory tree (`<base>/<pid>/<name>/{sock,bin}/`), removed on drop.
pub struct RuntimeRoot {
    root: PathBuf,
    sock_dir: PathBuf,
    bin_dir: PathBuf,
}

impl RuntimeRoot {
    /// Resolves a runtime root under `parent/<pid>/<name>/`, falling back on
    /// Unix to `/tmp/orzma-webview` when the socket path would overflow the
    /// `sun_path` limit; on Windows an overflow is an error.
    pub fn resolve_in(parent: &Path, pid: u32, name: &str) -> Result<Self, RuntimeRootError> {
        if socket_path_fits(parent, pid, name) {
            return Self::new_in(parent, pid, name);
        }
        #[cfg(unix)]
        {
            // NOTE: the shared fallback parent is created with the process umask (so
            // it is world-listable, like the legacy /tmp/orzma); only the per-handle
            // subdir below is 0700, which is what protects the sockets.
            let fallback = Path::new("/tmp/orzma-webview");
            std::fs::create_dir_all(fallback)?;
            if socket_path_fits(fallback, pid, name) {
                return Self::new_in(fallback, pid, name);
            }
        }
        Err(RuntimeRootError::SocketPathTooLong {
            name: name.to_owned(),
            limit: SUN_PATH_MAX,
        })
    }

    /// The socket path for the given `name` under this root.
    pub fn socket_path(&self, name: &str) -> PathBuf {
        self.sock_dir.join(format!("{name}.sock"))
    }

    /// The runtime root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding webview sockets.
    pub fn sock_dir(&self) -> &Path {
        &self.sock_dir
    }

    /// The directory holding command shims.
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    fn new_in(parent: &Path, pid: u32, name: &str) -> Result<Self, RuntimeRootError> {
        let root = parent.join(pid.to_string()).join(name);
        let sock_dir = root.join("sock");
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&sock_dir)?;
        std::fs::create_dir_all(&bin_dir)?;
        for dir in [&root, &sock_dir, &bin_dir] {
            restrict_to_current_user(dir)?;
        }
        // NOTE: the intermediate `<parent>/<pid>` dir is created by
        // `create_dir_all` with the process default permissions (world-listable
        // on Unix); restrict it too so handle names under it do not leak.
        if let Some(pid_dir) = root.parent() {
            restrict_to_current_user(pid_dir)?;
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

/// Whether the longest socket filename a webview uses
/// (`<name>.handlers.sock`) fits under `parent`. The `sun_path` field
/// is `SUN_PATH_MAX` bytes including the NUL terminator, so the path
/// itself must be shorter than that.
fn socket_path_fits(parent: &Path, pid: u32, name: &str) -> bool {
    parent
        .join(pid.to_string())
        .join(name)
        .join("sock")
        .join(format!("{name}.handlers.sock"))
        .as_os_str()
        .len()
        < SUN_PATH_MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_dir::assert_private_dir;

    /// Asserts that the socket directory is private to the current user
    /// and the whole tree is removed on drop.
    ///
    /// Case: orzma starts, mints its control-socket directory, and later
    /// exits.
    #[test]
    fn runtime_root_creates_sock_dir_private_and_drops() {
        let parent = tempfile::tempdir().unwrap();
        let path = {
            let rt = RuntimeRoot::resolve_in(parent.path(), 4242, "hello").unwrap();
            assert_private_dir(rt.sock_dir());
            assert_eq!(rt.socket_path("hello").parent().unwrap(), rt.sock_dir());
            rt.root().to_path_buf()
        };
        assert!(!path.exists(), "Drop must remove the tree");
    }

    /// Asserts that the intermediate `<pid>` directory is private to the
    /// current user.
    ///
    /// Case: another user lists the shared temp directory and must not
    /// learn which webview handles this orzma has minted.
    #[test]
    fn runtime_root_creates_pid_dir_private() {
        let parent = tempfile::tempdir().unwrap();
        let rt = RuntimeRoot::resolve_in(parent.path(), 4242, "hello").unwrap();
        assert_private_dir(rt.root().parent().unwrap());
    }

    /// Asserts that the command-shim directory is private to the current
    /// user.
    ///
    /// Case: a pane's `PATH` gains the shim directory; nobody else may
    /// plant executables there.
    #[test]
    fn runtime_root_creates_bin_dir_private() {
        let parent = tempfile::tempdir().unwrap();
        let rt = RuntimeRoot::resolve_in(parent.path(), 4243, "memo").unwrap();
        assert_private_dir(rt.bin_dir());
    }

    /// Asserts that two handles of one process get separate roots and
    /// dropping one leaves the other's sockets in place.
    ///
    /// Case: a program unregisters one webview while another of its
    /// webviews stays mounted.
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
                "same-PID webviews must not share a root"
            );
        } // b dropped here
        assert!(
            a_sock.exists(),
            "dropping one webview must not remove another's sockets"
        );
    }

    /// Asserts that a parent too deep for `sun_path` falls back to
    /// `/tmp/orzma-webview`.
    ///
    /// Case: macOS puts `$TMPDIR` under a long `/var/folders/…` path.
    #[cfg(unix)]
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

    /// Asserts that a handle name too long for `sun_path` even under the
    /// fallback parent is refused with `SocketPathTooLong`.
    ///
    /// Case: a program registers a webview under a very long name.
    #[test]
    fn runtime_root_errors_when_even_tmp_fallback_overflows() {
        let long_name = "n".repeat(60);
        let parent = tempfile::tempdir().unwrap();
        assert!(matches!(
            RuntimeRoot::resolve_in(parent.path(), 1, &long_name),
            Err(RuntimeRootError::SocketPathTooLong { .. })
        ));
    }

    /// Asserts that a socket path of exactly `SUN_PATH_MAX` bytes does not
    /// fit while one byte shorter does, since the kernel needs one byte
    /// for the NUL terminator.
    ///
    /// Case: a temp directory whose length puts the longest socket path
    /// right at the limit.
    #[test]
    fn a_socket_path_of_exactly_sun_path_max_bytes_does_not_fit() {
        let name = "n".repeat((SUN_PATH_MAX - 26) / 2);
        let len = |parent: &Path| {
            parent
                .join("1")
                .join(&name)
                .join("sock")
                .join(format!("{name}.handlers.sock"))
                .as_os_str()
                .len()
        };
        assert_eq!(len(Path::new("/pp")), SUN_PATH_MAX);
        assert!(!socket_path_fits(Path::new("/pp"), 1, &name));
        assert!(socket_path_fits(Path::new("/p"), 1, &name));
    }
}
