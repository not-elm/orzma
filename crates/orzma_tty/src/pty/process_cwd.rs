//! The working directory of the process a PTY is showing, read from the
//! operating system.

#[cfg(target_os = "macos")]
use libc::{
    PROC_PIDVNODEPATHINFO, c_int, c_void, pid_t, proc_listchildpids, proc_pidinfo,
    proc_vnodepathinfo,
};
#[cfg(target_os = "macos")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::fs::read_link;
use std::io::Result as IoResult;
#[cfg(not(target_os = "linux"))]
use std::io::{Error as IoError, ErrorKind};
#[cfg(target_os = "macos")]
use std::mem::MaybeUninit;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(windows)]
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tracing::debug;

/// The working directory of the first candidate process the OS reports
/// as a directory that exists and can be entered, or `None` when no
/// candidate yields one.
///
/// The candidates are `leader`, then `child`, then on macOS the children
/// of `child` when `child_is_wrapper` is set; each pid is queried at most
/// once.
pub(crate) fn resolve(
    leader: Option<i32>,
    child: Option<i32>,
    child_is_wrapper: bool,
) -> Option<PathBuf> {
    let wrapper = child.filter(|_| child_is_wrapper);
    let spawned = child
        .into_iter()
        .chain(wrapper.into_iter().flat_map(wrapper_children))
        .filter(|&pid| Some(pid) != leader);
    leader.into_iter().chain(spawned).find_map(enterable_cwd)
}

/// The pids of up to [`MAX_CHILDREN`] children of `pid`, in the order the
/// kernel lists them; a listing the kernel refuses yields no children.
#[cfg(target_os = "macos")]
fn wrapper_children(pid: i32) -> Vec<i32> {
    const PIDS_SIZE: c_int = (MAX_CHILDREN * size_of::<pid_t>()) as c_int;
    let mut pids: [pid_t; MAX_CHILDREN] = [0; MAX_CHILDREN];
    // SAFETY: `pids` is a writable allocation of exactly `PIDS_SIZE` bytes,
    // and `proc_listchildpids` writes at most `PIDS_SIZE` bytes into it.
    let count = unsafe { proc_listchildpids(pid, pids.as_mut_ptr().cast::<c_void>(), PIDS_SIZE) };
    pids.into_iter()
        .take(usize::try_from(count).unwrap_or(0))
        .collect()
}

/// Returns no children.
#[cfg(not(target_os = "macos"))]
fn wrapper_children(_pid: i32) -> Vec<i32> {
    Vec::new()
}

/// The working directory of `pid` when it can be read, still exists, and
/// can be entered; the reason is logged at debug level otherwise.
fn enterable_cwd(pid: i32) -> Option<PathBuf> {
    // NOTE: on Unix, `<dir>/.` resolves only with search permission on the
    // directory itself, which the spawn's chdir also needs; `is_dir()` on
    // the bare path would accept a directory the new shell cannot enter,
    // and the split would then fail to spawn. On Windows the `.` component
    // is collapsed before the syscall, so this check is equivalent there to
    // `path.is_dir()`.
    match read_cwd(pid) {
        Ok(path) if path.join(".").is_dir() => Some(path),
        Ok(path) => {
            debug!(pid, ?path, "working directory is gone or cannot be entered");
            None
        }
        Err(err) => {
            debug!(pid, %err, "working directory unreadable");
            None
        }
    }
}

/// The working directory of `pid` as the kernel records it.
///
/// # Errors
///
/// Returns the OS error when the process does not exist or belongs to
/// another user, and `InvalidData` when the answer is truncated or its
/// path is not NUL-terminated.
#[cfg(target_os = "macos")]
fn read_cwd(pid: i32) -> IoResult<PathBuf> {
    const INFO_SIZE: c_int = size_of::<proc_vnodepathinfo>() as c_int;
    let mut info = MaybeUninit::<proc_vnodepathinfo>::uninit();
    // SAFETY: `info` is a writable allocation of exactly `INFO_SIZE` bytes,
    // and `proc_pidinfo` writes at most `INFO_SIZE` bytes into it.
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            INFO_SIZE,
        )
    };
    if written <= 0 {
        return Err(IoError::last_os_error());
    }
    if written != INFO_SIZE {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            format!("proc_pidinfo wrote {written} of {INFO_SIZE} bytes"),
        ));
    }
    // SAFETY: `proc_pidinfo` reported writing all `INFO_SIZE` bytes of
    // `info`.
    let info = unsafe { info.assume_init() };
    let path = info.pvi_cdir.vip_path.as_flattened();
    let Some(len) = path.iter().position(|&c| c == 0) else {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            "the working directory path is not NUL-terminated",
        ));
    };
    let bytes: Vec<u8> = path[..len].iter().map(|c| c.cast_unsigned()).collect();
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

/// The working directory of `pid` as the kernel records it.
///
/// # Errors
///
/// Returns the OS error when the process does not exist or its
/// directory may not be read.
#[cfg(target_os = "linux")]
fn read_cwd(pid: i32) -> IoResult<PathBuf> {
    read_link(format!("/proc/{pid}/cwd"))
}

/// Reports that another process's working directory cannot be read here.
///
/// # Errors
///
/// Always returns `Unsupported`.
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn read_cwd(_pid: i32) -> IoResult<PathBuf> {
    Err(IoError::from(ErrorKind::Unsupported))
}

/// The working directory of `pid` as the operating system records it,
/// without the trailing separator the loader stores.
///
/// # Errors
///
/// Returns `InvalidInput` when `pid` does not fit a Windows process id,
/// and `NotFound` when the process is not listed or its directory cannot
/// be read — which is the case for a process this one may not read the
/// memory of, such as an elevated shell or one owned by another user.
#[cfg(windows)]
fn read_cwd(pid: i32) -> IoResult<PathBuf> {
    let pid = u32::try_from(pid)
        .map(Pid::from_u32)
        .map_err(|_| IoError::from(ErrorKind::InvalidInput))?;
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    system
        .process(pid)
        .and_then(Process::cwd)
        .map(trimmed)
        .ok_or_else(|| IoError::from(ErrorKind::NotFound))
}

/// `path` without the trailing separator, unless the path is a drive
/// root, where that separator is what makes it absolute.
///
/// The NT loader stores `CurrentDirectory.DosPath` with a trailing
/// separator, and `sysinfo` passes it through unchanged.
// NOTE: trimming a drive root would turn `C:\` into `C:`, which names
// the current directory on that drive rather than its root, so a split
// would land somewhere else entirely.
#[cfg(windows)]
fn trimmed(path: &Path) -> PathBuf {
    // NOTE: a path the loader stored as UTF-16 can hold an unpaired
    // surrogate, which no `&str` can carry. Going through
    // `to_string_lossy` would replace it and name a different directory,
    // so a path that is not UTF-8 is passed through untouched.
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.len() <= 2 {
        return path.to_path_buf();
    }
    PathBuf::from(trimmed)
}

/// How many children of a wrapper process are tried.
#[cfg(target_os = "macos")]
const MAX_CHILDREN: usize = 64;

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    /// Asserts that the working directory read for a live process is the
    /// one it was started in, with no trailing separator.
    ///
    /// Case: a pane's shell is sitting at a prompt in a project
    /// directory and the user splits that pane.
    #[test]
    fn read_cwd_reports_the_directory_a_process_was_started_in() {
        let dir = TempDir::new().expect("a temporary directory");
        // NOTE: `canonicalize` returns a verbatim `\\?\C:\…` path, whose
        // `VerbatimDisk` prefix `Path` compares unequal to the plain
        // `Disk` prefix the PEB stores. Compare the raw path instead.
        let expected = dir.path().to_path_buf();
        // NOTE: `cmd /c pause` exits immediately when stdin is not a
        // console, which is how `cargo test` usually runs it, so the
        // child must stay alive without reading stdin.
        let mut child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .current_dir(&expected)
            .stdout(Stdio::null())
            .spawn()
            .expect("a spawned process");
        let pid = i32::try_from(child.id()).expect("a pid that fits");
        let read = read_cwd(pid);
        let _ = child.kill();
        let _ = child.wait();
        let read = read.expect("a readable working directory");
        assert_eq!(read, expected);
        assert!(
            !read.as_os_str().to_string_lossy().ends_with('\\'),
            "the trailing separator must be trimmed"
        );
    }

    /// Asserts that a pid no process holds reports an error rather than
    /// a directory.
    ///
    /// Case: the pane's shell exits between the moment its pid was
    /// recorded and the moment the split asks for its directory.
    #[test]
    fn read_cwd_of_a_dead_process_is_an_error() {
        let mut child = Command::new("cmd")
            .args(["/c", "exit"])
            .spawn()
            .expect("a spawned process");
        let pid = i32::try_from(child.id()).expect("a pid that fits");
        let _ = child.wait();
        assert!(read_cwd(pid).is_err());
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::pty::tests::holds_within;
    use std::fs::{Permissions, set_permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    #[cfg(target_os = "macos")]
    use std::process::Stdio;
    use std::process::{Child, Command};
    #[cfg(target_os = "macos")]
    use std::time::Duration;
    use tempfile::TempDir;

    /// A process started as the leader of its own process group, whose
    /// group is killed and which is reaped when dropped.
    struct Running(Child);

    impl Running {
        fn start(command: &mut Command) -> Self {
            Self(
                command
                    .process_group(0)
                    .spawn()
                    .expect("the process starts"),
            )
        }

        fn sleep_in(dir: &Path) -> Self {
            Self::start(Command::new("sleep").arg("30").current_dir(dir))
        }

        /// A `/bin/sh` started in `dir` that runs `sleep` in `job_dir` as a
        /// background job and waits for it.
        #[cfg(target_os = "macos")]
        fn shell_with_job_in(dir: &Path, job_dir: &Path) -> Self {
            let script = format!("(cd '{}' && exec sleep 30) & wait", job_dir.display());
            Self::start(
                Command::new("/bin/sh")
                    .arg("-c")
                    .arg(script)
                    .current_dir(dir)
                    .stderr(Stdio::null()),
            )
        }

        fn pid(&self) -> i32 {
            i32::try_from(self.0.id()).expect("a pid fits in i32")
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let _ = Command::new("pkill")
                .arg("-KILL")
                .arg("-g")
                .arg(self.0.id().to_string())
                .status();
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Asserts that the foreground leader's directory is reported ahead
    /// of the spawned process's.
    ///
    /// Case: the user runs a program in another directory in the pane's
    /// foreground while the shell waits in its own directory.
    #[test]
    fn the_foreground_leader_is_tried_before_the_spawned_process() {
        let leader_dir = TempDir::new().expect("a temp dir");
        let child_dir = TempDir::new().expect("a temp dir");
        let leader = Running::sleep_in(leader_dir.path());
        let child = Running::sleep_in(child_dir.path());
        let expected = leader_dir
            .path()
            .canonicalize()
            .expect("the dir canonicalizes");
        assert_eq!(
            resolve(Some(leader.pid()), Some(child.pid()), false),
            Some(expected)
        );
    }

    /// Asserts that a leader pid with no process behind it falls through
    /// to the spawned process.
    ///
    /// Case: the program that led the foreground pipeline has exited
    /// while the shell still waits for the rest of the pipeline.
    #[test]
    fn a_leader_that_no_longer_exists_falls_back_to_the_spawned_process() {
        let child_dir = TempDir::new().expect("a temp dir");
        let child = Running::sleep_in(child_dir.path());
        let expected = child_dir
            .path()
            .canonicalize()
            .expect("the dir canonicalizes");
        assert_eq!(
            resolve(Some(i32::MAX), Some(child.pid()), false),
            Some(expected)
        );
    }

    /// Asserts that a leader whose working directory was removed is
    /// skipped rather than reported.
    ///
    /// Case: a program keeps running in the foreground after the
    /// directory it was started in is deleted from another pane.
    #[test]
    fn a_leader_whose_directory_was_removed_falls_back_to_the_spawned_process() {
        let leader_dir = TempDir::new().expect("a temp dir");
        let child_dir = TempDir::new().expect("a temp dir");
        let leader = Running::sleep_in(leader_dir.path());
        let child = Running::sleep_in(child_dir.path());
        let expected = child_dir
            .path()
            .canonicalize()
            .expect("the dir canonicalizes");
        leader_dir
            .close()
            .expect("the leader's directory is removed");
        assert_eq!(
            resolve(Some(leader.pid()), Some(child.pid()), false),
            Some(expected)
        );
    }

    /// Asserts that a leader whose working directory can no longer be
    /// entered is skipped rather than reported.
    ///
    /// Case: a program keeps running in the foreground after the user
    /// removes the search permission from the directory it was started in.
    #[test]
    fn a_leader_whose_directory_cannot_be_entered_falls_back_to_the_spawned_process() {
        let leader_dir = TempDir::new().expect("a temp dir");
        let child_dir = TempDir::new().expect("a temp dir");
        let leader = Running::sleep_in(leader_dir.path());
        let child = Running::sleep_in(child_dir.path());
        let expected = child_dir
            .path()
            .canonicalize()
            .expect("the dir canonicalizes");
        set_permissions(leader_dir.path(), Permissions::from_mode(0o000))
            .expect("the leader's directory is locked");
        let still_enterable = leader_dir.path().join(".").is_dir();
        let resolved = resolve(Some(leader.pid()), Some(child.pid()), false);
        set_permissions(leader_dir.path(), Permissions::from_mode(0o700))
            .expect("the leader's directory is unlocked");
        // NOTE: root enters a directory whatever its mode, so the locked
        // directory cannot be set up when the tests run as root.
        if still_enterable {
            return;
        }
        assert_eq!(resolved, Some(expected));
    }

    /// Asserts that a child of a spawned wrapper process is tried when the
    /// wrapper itself yields no directory.
    ///
    /// Case: on macOS the pane's process is the `login` wrapper, whose
    /// directory the user cannot read, and the shell is its child.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_child_of_a_wrapper_is_tried_last() {
        let outer = TempDir::new().expect("a temp dir");
        let inner = TempDir::new().expect("a temp dir");
        let expected = inner.path().canonicalize().expect("the dir canonicalizes");
        let wrapper = Running::shell_with_job_in(outer.path(), &expected);
        outer.close().expect("the wrapper's directory is removed");
        let mut found = None;
        holds_within(
            || {
                found = resolve(None, Some(wrapper.pid()), true);
                found.as_ref() == Some(&expected)
            },
            Duration::from_secs(10),
        );
        assert_eq!(found, Some(expected));
    }

    /// Asserts that the children of a spawned process that is not a
    /// wrapper are not tried when the process itself yields no directory.
    ///
    /// Case: on macOS the shell was spawned directly because `$USER` or
    /// `$HOME` was unset, its directory was deleted, and a background job
    /// it started still runs in another directory.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_children_of_a_process_that_is_not_a_wrapper_are_not_tried() {
        let outer = TempDir::new().expect("a temp dir");
        let inner = TempDir::new().expect("a temp dir");
        let job_dir = inner.path().canonicalize().expect("the dir canonicalizes");
        let shell = Running::shell_with_job_in(outer.path(), &job_dir);
        outer.close().expect("the shell's directory is removed");
        let job_entered_its_directory = holds_within(
            || resolve(None, Some(shell.pid()), true).as_ref() == Some(&job_dir),
            Duration::from_secs(10),
        );
        assert!(
            job_entered_its_directory,
            "the background job never entered its directory"
        );
        assert_eq!(resolve(None, Some(shell.pid()), false), None);
    }

    /// Asserts that no leader and no spawned process yield no directory.
    ///
    /// Case: the pane's terminal is built around a fake master with no
    /// process behind it.
    #[test]
    fn no_candidates_yield_no_directory() {
        assert_eq!(resolve(None, None, false), None);
    }
}
