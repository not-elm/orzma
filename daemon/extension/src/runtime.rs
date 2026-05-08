// daemon/extension/src/runtime.rs
use std::{fs, path::{Path, PathBuf}};
#[cfg(unix)]
use libc;

pub struct RuntimeRoot {
    root: PathBuf,
    bin_dir: PathBuf,
    sock_dir: PathBuf,
}

impl RuntimeRoot {
    pub fn new_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        let root = parent.join(pid.to_string());
        let bin_dir = root.join("bin");
        let sock_dir = root.join("sock");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&bin_dir)?;
        fs::create_dir_all(&sock_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&root, &bin_dir, &sock_dir] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self { root, bin_dir, sock_dir })
    }

    pub fn root(&self) -> &Path { &self.root }
    pub fn bin_dir(&self) -> &Path { &self.bin_dir }
    pub fn sock_dir(&self) -> &Path { &self.sock_dir }
}

const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };

impl RuntimeRoot {
    /// Resolve a runtime root under `parent`/`<pid>/`, falling back to `/tmp/`
    /// when the resulting socket path would overflow the platform's sun_path limit.
    pub fn resolve_in(parent: &Path, pid: u32, longest_extension_name: &str) -> std::io::Result<Self> {
        let needed = |base: &Path| -> usize {
            base.join(pid.to_string()).join("sock")
                .join(format!("{longest_extension_name}.sock"))
                .as_os_str().len()
        };
        if needed(parent) <= SUN_PATH_MAX {
            return Self::new_in(parent, pid);
        }
        let fallback = Path::new("/tmp/ozmux");
        std::fs::create_dir_all(fallback)?;
        if needed(fallback) <= SUN_PATH_MAX {
            return Self::new_in(fallback, pid);
        }
        Err(std::io::Error::other(format!(
            "extension name '{longest_extension_name}' produces a socket path longer than {SUN_PATH_MAX} bytes"
        )))
    }
}

impl RuntimeRoot {
    /// Remove `<parent>/<pid>/` subtrees whose pid is no longer running.
    /// Non-numeric entries are left untouched.
    pub fn gc_stale(parent: &Path) -> std::io::Result<()> {
        let dir = match std::fs::read_dir(parent) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else { continue };
            let Ok(pid) = name_str.parse::<u32>() else { continue };
            if !pid_is_alive(pid) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // kill(pid, 0): 0 → alive, ESRCH → dead, EPERM → alive but other-owner.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 { return true; }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

impl Drop for RuntimeRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn new_in_creates_subdirs_with_0700() {
        let parent = tempdir().unwrap();
        let rt = RuntimeRoot::new_in(parent.path(), 12345).unwrap();
        for p in [rt.root(), rt.bin_dir(), rt.sock_dir()] {
            let mode = fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "wrong mode on {:?}", p);
        }
    }

    #[test]
    fn drop_removes_tree() {
        let parent = tempdir().unwrap();
        let path = {
            let rt = RuntimeRoot::new_in(parent.path(), 99999).unwrap();
            rt.root().to_path_buf()
        };
        assert!(!path.exists(), "tree should be removed by Drop");
    }

    #[test]
    fn resolve_picks_tmpdir_when_path_fits() {
        let tmp = tempdir().unwrap();
        let rt = RuntimeRoot::resolve_in(tmp.path(), 1, "memo").unwrap();
        assert!(rt.root().starts_with(tmp.path()));
    }

    #[test]
    fn resolve_falls_back_to_slash_tmp_when_tmpdir_too_long() {
        // Make a parent path so long that adding "/<pid>/sock/<ext>.sock" overflows sun_path.
        let deep = std::iter::repeat("a").take(120).collect::<Vec<_>>().join("/");
        let outer = tempdir().unwrap();
        let tmp = outer.path().join(deep);
        std::fs::create_dir_all(&tmp).unwrap();
        let rt = RuntimeRoot::resolve_in(&tmp, 7, "ext").unwrap();
        assert!(
            rt.root().starts_with("/tmp"),
            "expected /tmp fallback, got {:?}",
            rt.root()
        );
    }

    #[test]
    fn gc_removes_stale_pid_dirs() {
        let parent = tempdir().unwrap();
        let live_pid = std::process::id();
        let candidate: u32 = 999_999_999;
        let stale_pid = if unsafe { libc::kill(candidate as i32, 0) } == -1 { candidate } else { 1 };

        std::fs::create_dir_all(parent.path().join(live_pid.to_string())).unwrap();
        std::fs::create_dir_all(parent.path().join(stale_pid.to_string())).unwrap();
        std::fs::create_dir_all(parent.path().join("not-a-pid")).unwrap();

        RuntimeRoot::gc_stale(parent.path()).unwrap();

        assert!(parent.path().join(live_pid.to_string()).exists());
        assert!(!parent.path().join(stale_pid.to_string()).exists());
        assert!(parent.path().join("not-a-pid").exists()); // non-numeric names left alone
    }
}
