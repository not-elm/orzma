//! The working directory of the process a PTY is showing, read from the
//! operating system.

#[cfg(target_os = "macos")]
use libc::{
    PROC_PIDVNODEPATHINFO, c_int, c_void, pid_t, proc_listchildpids, proc_pidinfo,
    proc_vnodepathinfo,
};
#[cfg(target_os = "macos")]
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::fs::read_link;
use std::io::Result as IoResult;
#[cfg(not(target_os = "linux"))]
use std::io::{Error as IoError, ErrorKind};
#[cfg(target_os = "macos")]
use std::mem::MaybeUninit;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use tracing::debug;

/// The working directory of the first candidate process the OS reports
/// as an existing directory, or `None` when no candidate yields one.
///
/// The candidates are `leader`, then `child`, then on macOS the children
/// of `child`; each pid is queried at most once, and a `child` pid that
/// does not fit a `pid_t` is skipped.
pub(crate) fn resolve(leader: Option<i32>, child: Option<u32>) -> Option<PathBuf> {
    let child = child.and_then(|pid| i32::try_from(pid).ok());
    let mut tried = Vec::new();
    leader
        .into_iter()
        .chain(child)
        .chain(child.into_iter().flat_map(wrapper_children))
        .find_map(|pid| {
            if tried.contains(&pid) {
                return None;
            }
            tried.push(pid);
            existing_cwd(pid)
        })
}

/// The children of `pid`, or none when they cannot be listed; the
/// failure is logged at debug level.
#[cfg(target_os = "macos")]
fn wrapper_children(pid: i32) -> Vec<i32> {
    child_pids(pid).unwrap_or_else(|err| {
        debug!(pid, %err, "listing child processes failed");
        Vec::new()
    })
}

/// Returns no children.
#[cfg(not(target_os = "macos"))]
fn wrapper_children(_pid: i32) -> Vec<i32> {
    Vec::new()
}

/// The working directory of `pid` when it can be read and still exists;
/// the reason is logged at debug level otherwise.
fn existing_cwd(pid: i32) -> Option<PathBuf> {
    match read_cwd(pid) {
        Ok(path) if path.is_dir() => Some(path),
        Ok(path) => {
            debug!(pid, ?path, "working directory no longer exists");
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
    Ok(PathBuf::from(OsStr::from_bytes(&bytes)))
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
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_cwd(_pid: i32) -> IoResult<PathBuf> {
    Err(IoError::from(ErrorKind::Unsupported))
}

/// The pids of up to [`MAX_CHILDREN`] children of `pid`, in the order
/// the kernel lists them.
///
/// # Errors
///
/// Returns the OS error when the children cannot be listed.
#[cfg(target_os = "macos")]
fn child_pids(pid: i32) -> IoResult<Vec<i32>> {
    const PIDS_SIZE: c_int = (MAX_CHILDREN * size_of::<pid_t>()) as c_int;
    let mut pids: [pid_t; MAX_CHILDREN] = [0; MAX_CHILDREN];
    // SAFETY: `pids` is a writable allocation of exactly `PIDS_SIZE` bytes,
    // and `proc_listchildpids` writes at most `PIDS_SIZE` bytes into it.
    let count = unsafe { proc_listchildpids(pid, pids.as_mut_ptr().cast::<c_void>(), PIDS_SIZE) };
    let Ok(count) = usize::try_from(count) else {
        return Err(IoError::last_os_error());
    };
    Ok(pids.into_iter().take(count).collect())
}

/// How many children of the spawned process are tried.
#[cfg(target_os = "macos")]
const MAX_CHILDREN: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    #[cfg(target_os = "macos")]
    use std::process::Stdio;
    use std::process::{Child, Command};
    #[cfg(target_os = "macos")]
    use std::thread;
    #[cfg(target_os = "macos")]
    use std::time::{Duration, Instant};
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
            resolve(Some(leader.pid()), Some(child.0.id())),
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
        assert_eq!(resolve(Some(i32::MAX), Some(child.0.id())), Some(expected));
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
            resolve(Some(leader.pid()), Some(child.0.id())),
            Some(expected)
        );
    }

    /// Asserts that a child of the spawned process is tried when the
    /// spawned process itself yields no directory.
    ///
    /// Case: on macOS the pane's process is the `login` wrapper, whose
    /// directory the user cannot read, and the shell is its child.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_child_of_the_spawned_process_is_tried_last() {
        let outer = TempDir::new().expect("a temp dir");
        let inner = TempDir::new().expect("a temp dir");
        let expected = inner.path().canonicalize().expect("the dir canonicalizes");
        let script = format!("(cd '{}' && exec sleep 30) & wait", expected.display());
        let wrapper = Running::start(
            Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .current_dir(outer.path())
                .stderr(Stdio::null()),
        );
        outer.close().expect("the wrapper's directory is removed");
        let deadline = Instant::now() + Duration::from_secs(10);
        let found = loop {
            let found = resolve(None, Some(wrapper.0.id()));
            if found.as_ref() == Some(&expected) || Instant::now() >= deadline {
                break found;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(found, Some(expected));
    }

    /// Asserts that no leader and no spawned process yield no directory.
    ///
    /// Case: the pane's terminal is built around a fake master with no
    /// process behind it.
    #[test]
    fn no_candidates_yield_no_directory() {
        assert_eq!(resolve(None, None), None);
    }
}
