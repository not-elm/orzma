//! Per-process CEF profile directory: a unique `root_cache_path` per orzma
//! instance so concurrent instances never collide on Chromium's per-profile
//! singleton lock.

use bevy_orzma_webview_host::restrict_to_current_user;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

/// A per-process CEF profile directory (`$TMPDIR/orzma-cef/<pid>/`), removed on drop.
///
/// Chromium's `ProcessSingleton` permits only one live process per profile
/// directory, so a shared profile makes a second orzma instance fail. Keying the
/// directory by PID guarantees concurrent instances never collide, since live
/// PIDs are unique.
pub(crate) struct CefProfileDir {
    path: PathBuf,
}

impl CefProfileDir {
    /// Sweeps stale per-PID profile directories (dead owners) under the shared
    /// base, then creates and claims this process's own profile directory.
    pub(crate) fn acquire() -> std::io::Result<Self> {
        let base = std::env::temp_dir().join("orzma-cef");
        std::fs::create_dir_all(&base)?;
        restrict_to_current_user(&base)?;
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
        restrict_to_current_user(&path)?;
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
    // NOTE: kill(0, …) and kill(<negative>, …) target a process GROUP, not one
    // PID; a directory named 0 or above pid_t's positive range would otherwise
    // misclassify liveness and risk sweeping the wrong directory. Real PIDs are
    // 1..=i32::MAX — treat anything outside that as alive so it is never swept.
    if pid == 0 || pid > i32::MAX as u32 {
        return true;
    }
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

/// Whether `pid` names a live process, deciding "alive" whenever the
/// answer is not a clear no.
///
/// PID 0 (the System Idle Process) is never probed. `OpenProcess` failing
/// with `ERROR_INVALID_PARAMETER` is the one "no such process" answer;
/// any other failure (access denied for another user's or a protected
/// process) counts as alive. A signaled process handle means the process
/// has exited. Windows reuses PIDs quickly, so a directory left by a dead
/// process whose PID a live process now holds stays until that process
/// exits — the conservative direction.
#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    // SAFETY: `OpenProcess` has no preconditions; a null result is handled
    // below and a non-null one is wrapped so it is closed exactly once.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        // NOTE: misclassifying a live PID as dead would let the sweep delete a
        // running instance's profile directory, so only the documented
        // "no such process" error may return false.
        return std::io::Error::last_os_error().raw_os_error()
            != Some(ERROR_INVALID_PARAMETER as i32);
    }
    // SAFETY: `raw` is a valid handle `OpenProcess` just returned and nothing
    // else owns it.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the handle is valid for the call, and a zero timeout never blocks.
    let waited = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
    waited != WAIT_OBJECT_0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use bevy_orzma_webview_host::private_dir::security_descriptor_sddl;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// Asserts that the profile directory is private to the current user and
    /// removed on drop.
    ///
    /// Case: orzma starts, claims its CEF profile directory, and exits.
    #[test]
    fn resolve_in_creates_a_private_dir_and_drops() {
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
            #[cfg(windows)]
            {
                let sddl = security_descriptor_sddl(profile.path()).unwrap();
                assert!(
                    sddl.starts_with("D:P"),
                    "the DACL must be protected: {sddl}"
                );
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

    /// Asserts that a running process is reported alive.
    ///
    /// Case: two orzma instances run side by side and one sweeps the
    /// shared profile base while the other is still up.
    #[cfg(windows)]
    #[test]
    fn pid_alive_reports_the_current_process_alive() {
        assert!(pid_alive(std::process::id()));
    }

    /// Asserts that a process that has exited is reported dead.
    ///
    /// Case: an earlier orzma instance crashed and left its profile
    /// directory behind.
    #[cfg(windows)]
    #[test]
    fn pid_alive_reports_an_exited_process_dead() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid));
    }

    /// Asserts that PID 0 is treated as alive rather than probed.
    ///
    /// Case: a stray directory named `0` sits under the profile base.
    #[cfg(windows)]
    #[test]
    fn pid_alive_treats_pid_zero_as_alive() {
        assert!(pid_alive(0));
    }
}
