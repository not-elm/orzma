//! PID file management for the ozmux daemon. Tracks `$TMPDIR/ozmux/daemon.pid`
//! so external tooling (e.g. `ozmux daemon stop`) can discover the running
//! daemon's PID.

use libc;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn path_under(parent: &Path) -> PathBuf {
    parent.join("daemon.pid")
}

pub(crate) fn write_to(path: &Path, pid: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, pid.to_string())
}

pub(crate) fn read_from(path: &Path) -> io::Result<Option<u32>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s.trim().parse::<u32>().ok()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn remove_at(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Returns `Ok(true)` if `kill(pid, 0)` succeeds or returns `EPERM`
/// (process exists but we can't signal it). Returns `Ok(false)` on
/// `ESRCH` (no such process) or when `pid` is not a valid `pid_t`. Any
/// other errno is propagated.
fn is_process_alive(pid: u32) -> io::Result<bool> {
    // NOTE: PIDs must fit in a positive pid_t (i32). 0 and any value
    // above i32::MAX are not valid process identifiers — kill() with
    // those values targets process groups or broadcasts instead, which
    // would turn a routine liveness check into a system-wide signal.
    // Treat them as "not alive" so cleanup_if_stale removes the
    // corrupted file rather than acting on it.
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(false);
    }
    // SAFETY: libc::kill with signal 0 has no side effects and is
    // documented as the standard liveness probe. The guard above
    // ensures `pid` fits in a positive pid_t, so the cast to i32 is
    // value-preserving.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(err),
    }
}

pub(crate) fn cleanup_if_stale_under(parent: &Path) -> io::Result<()> {
    let path = path_under(parent);
    let Some(pid) = read_from(&path)? else {
        return Ok(());
    };
    if !is_process_alive(pid)? {
        remove_at(&path)?;
    }
    Ok(())
}

fn default_parent() -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("ozmux");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Returns `$TMPDIR/ozmux/daemon.pid`, creating the parent directory if
/// needed.
pub fn path() -> io::Result<PathBuf> {
    Ok(path_under(&default_parent()?))
}

/// Writes the current daemon's PID to `$TMPDIR/ozmux/daemon.pid`.
pub fn write(pid: u32) -> io::Result<()> {
    write_to(&path()?, pid)
}

/// Reads the PID from `$TMPDIR/ozmux/daemon.pid`, or `None` if the file
/// does not exist.
pub fn read() -> io::Result<Option<u32>> {
    read_from(&path()?)
}

/// Removes `$TMPDIR/ozmux/daemon.pid`. Idempotent.
pub fn remove() -> io::Result<()> {
    remove_at(&path()?)
}

/// Removes the PID file if it references a process that no longer
/// exists. Called by `run()` at startup.
pub fn cleanup_if_stale() -> io::Result<()> {
    cleanup_if_stale_under(&default_parent()?)
}

/// RAII guard that removes the PID file on drop. Created by `run()` to
/// ensure cleanup on any unwind path (graceful shutdown, error
/// propagation, panic).
pub struct PidFileGuard;

impl PidFileGuard {
    /// Writes `pid` to `$TMPDIR/ozmux/daemon.pid` and returns a guard that
    /// removes the file on drop. Any I/O error from the write is propagated.
    pub fn create(pid: u32) -> io::Result<Self> {
        write(pid)?;
        Ok(Self)
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn path_under_appends_daemon_pid() {
        let dir = TempDir::new().unwrap();
        let p = path_under(dir.path());
        assert_eq!(p, dir.path().join("daemon.pid"));
    }

    #[test]
    fn write_then_read_round_trip() {
        let dir = TempDir::new().unwrap();
        write_to(&path_under(dir.path()), 12345).unwrap();
        let got = read_from(&path_under(dir.path())).unwrap();
        assert_eq!(got, Some(12345));
    }

    #[test]
    fn read_returns_none_when_absent() {
        let dir = TempDir::new().unwrap();
        let got = read_from(&path_under(dir.path())).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn remove_is_idempotent_when_absent() {
        let dir = TempDir::new().unwrap();
        remove_at(&path_under(dir.path())).unwrap();
    }

    #[test]
    fn remove_deletes_existing_file() {
        let dir = TempDir::new().unwrap();
        let p = path_under(dir.path());
        write_to(&p, 1).unwrap();
        remove_at(&p).unwrap();
        assert!(!p.exists());
    }

    #[test]
    fn cleanup_if_stale_removes_dead_pid_entry() {
        let dir = TempDir::new().unwrap();
        let p = path_under(dir.path());
        // NOTE: i32::MAX (as u32) is the largest positive value that survives
        // the cast to libc::pid_t (i32) and is far above any PID limit on
        // macOS (~99k) or Linux (theoretical max ~4M), so kill(0) reliably
        // returns ESRCH. We avoid u32::MAX because it wraps to -1, which
        // POSIX treats as "broadcast to all processes" rather than a lookup.
        // We also avoid small synthetic PIDs like 1 (init), which return
        // EPERM, which our code interprets as "alive".
        write_to(&p, i32::MAX as u32).unwrap();
        cleanup_if_stale_under(dir.path()).unwrap();
        assert!(!p.exists(), "stale PID file should have been removed");
    }

    #[test]
    fn cleanup_if_stale_keeps_live_pid_entry() {
        let dir = TempDir::new().unwrap();
        let p = path_under(dir.path());
        let me = std::process::id();
        write_to(&p, me).unwrap();
        cleanup_if_stale_under(dir.path()).unwrap();
        assert!(p.exists(), "live PID file should be left intact");
    }

    #[test]
    fn cleanup_if_stale_is_noop_when_absent() {
        let dir = TempDir::new().unwrap();
        cleanup_if_stale_under(dir.path()).unwrap();
    }

    #[test]
    fn cleanup_if_stale_removes_invalid_pid_zero() {
        let dir = TempDir::new().unwrap();
        let p = path_under(dir.path());
        write_to(&p, 0).unwrap();
        cleanup_if_stale_under(dir.path()).unwrap();
        assert!(
            !p.exists(),
            "PID 0 should be treated as invalid and the file removed"
        );
    }

    #[test]
    fn cleanup_if_stale_removes_pid_above_i32_max() {
        let dir = TempDir::new().unwrap();
        let p = path_under(dir.path());
        // NOTE: Any value > i32::MAX would cast to a negative pid_t and turn
        // kill() into a broadcast — reject it before that can happen.
        write_to(&p, (i32::MAX as u32) + 1).unwrap();
        cleanup_if_stale_under(dir.path()).unwrap();
        assert!(!p.exists(), "out-of-range PID should be treated as invalid");
    }
}
