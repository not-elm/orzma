//! Tokio-free host runtime: a per-handle runtime root used to mint the 0700
//! socket directory tree for the webview control plane.

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
    /// Resolves a runtime root under `parent/<pid>/<name>/`, falling back to
    /// `/tmp/orzma-webview` when the socket path would overflow the `sun_path` limit.
    pub fn resolve_in(parent: &Path, pid: u32, name: &str) -> Result<Self, RuntimeRootError> {
        // NOTE: measure the LONGEST socket filename a webview uses
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
        // it is world-listable, like the legacy /tmp/orzma); only the per-handle
        // subdir below is 0700, which is what protects the sockets.
        let fallback = Path::new("/tmp/orzma-webview");
        std::fs::create_dir_all(fallback)?;
        if needed(fallback) <= SUN_PATH_MAX {
            return Self::new_in(fallback, pid, name);
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
            "the intermediate <pid> dir must be 0700 so webview names do not leak"
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
                "same-PID webviews must not share a root"
            );
        } // b dropped here
        assert!(
            a_sock.exists(),
            "dropping one webview must not remove another's sockets"
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

    #[test]
    fn runtime_root_errors_when_even_tmp_fallback_overflows() {
        let long_name = "n".repeat(60);
        let parent = tempfile::tempdir().unwrap();
        assert!(matches!(
            RuntimeRoot::resolve_in(parent.path(), 1, &long_name),
            Err(RuntimeRootError::SocketPathTooLong { .. })
        ));
    }
}
