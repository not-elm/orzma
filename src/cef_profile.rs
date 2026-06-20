//! Per-process CEF profile directory: a unique `root_cache_path` per ozmux
//! instance so concurrent instances never collide on Chromium's per-profile
//! singleton lock.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A per-process CEF profile directory (`$TMPDIR/ozmux-cef/<pid>/`), removed on drop.
///
/// Chromium's `ProcessSingleton` permits only one live process per profile
/// directory, so a shared profile makes a second ozmux instance fail. Keying the
/// directory by PID guarantees concurrent instances never collide, since live
/// PIDs are unique.
pub(crate) struct CefProfileDir {
    path: PathBuf,
}

impl CefProfileDir {
    /// Sweeps stale per-PID profile directories (dead owners) under the shared
    /// base, then creates and claims this process's own profile directory.
    pub(crate) fn acquire() -> std::io::Result<Self> {
        let base = std::env::temp_dir().join("ozmux-cef");
        std::fs::create_dir_all(&base)?;
        #[cfg(unix)]
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
        let pid = std::process::id();
        sweep_in(&base, pid_alive, pid);
        Self::resolve_in(&base, pid)
    }

    /// The absolute path to pass to CEF as `root_cache_path`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn resolve_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        let path = parent.join(pid.to_string());
        // NOTE: no concurrent process can share our PID, so a pre-existing dir
        // here is a stale leftover from a dead same-PID process; removing it
        // keeps the profile freshly ephemeral. It must never inherit cross-run
        // state — a reused stale SingletonLock would otherwise mislead Chromium.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self { path })
    }
}

impl Drop for CefProfileDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn sweep_in(base: &Path, is_alive: impl Fn(u32) -> bool, self_pid: u32) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != self_pid && !is_alive(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 sends no signal; it performs only the
    // existence/permission check and has no preconditions on `pid`.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // NOTE: ESRCH means no such process (dead); any other errno (e.g. EPERM —
    // the process exists but is owned by another user) means alive.
    // Misclassifying a live PID as dead would let the sweep delete a running
    // instance's profile directory.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_in_creates_0700_dir_and_drops() {
        let parent = tempfile::tempdir().unwrap();
        let path = {
            let profile = CefProfileDir::resolve_in(parent.path(), 4242).unwrap();
            assert!(profile.path().is_absolute());
            assert_eq!(profile.path(), parent.path().join("4242"));
            #[cfg(unix)]
            {
                let mode = std::fs::metadata(profile.path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o700);
            }
            profile.path().to_path_buf()
        };
        assert!(!path.exists(), "Drop must remove the profile dir");
    }

    #[test]
    fn resolve_in_replaces_stale_same_pid_dir() {
        let parent = tempfile::tempdir().unwrap();
        let stale = parent.path().join("4243");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("SingletonLock"), b"stale").unwrap();

        let profile = CefProfileDir::resolve_in(parent.path(), 4243).unwrap();

        assert_eq!(profile.path(), stale);
        assert!(profile.path().exists());
        assert!(
            !profile.path().join("SingletonLock").exists(),
            "a fresh profile dir must not inherit the stale lock marker"
        );
        assert!(
            std::fs::read_dir(profile.path()).unwrap().next().is_none(),
            "the re-created profile dir must be empty"
        );
    }

    #[test]
    fn sweep_in_removes_dead_keeps_alive_and_self() {
        let base = tempfile::tempdir().unwrap();
        for pid in ["100", "200", "300"] {
            std::fs::create_dir_all(base.path().join(pid)).unwrap();
        }
        let is_alive = |pid: u32| pid == 100;

        sweep_in(base.path(), is_alive, 300);

        assert!(base.path().join("100").exists(), "alive owner kept");
        assert!(!base.path().join("200").exists(), "dead owner swept");
        assert!(base.path().join("300").exists(), "self never swept");
    }

    #[test]
    fn sweep_in_ignores_non_numeric_entries() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("not-a-pid")).unwrap();
        std::fs::write(base.path().join("README"), b"x").unwrap();
        std::fs::create_dir_all(base.path().join("200")).unwrap();
        let is_alive = |_pid: u32| false;

        sweep_in(base.path(), is_alive, 999);

        assert!(
            base.path().join("not-a-pid").exists(),
            "non-numeric dir untouched"
        );
        assert!(base.path().join("README").exists(), "stray file untouched");
        assert!(
            !base.path().join("200").exists(),
            "numeric dead owner swept"
        );
    }
}
