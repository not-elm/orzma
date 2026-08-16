//! `Pty` — owns the PTY master, writer, child killer, and the

use crate::{
    SpawnOptions,
    error::{OrzmaTermError, OrzmaTermResult},
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Result as IoResult;
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

/// PTY ownership for one spawned shell.
///
/// `Mutex` is required because `dyn MasterPty + Send` and `dyn Write +
/// Send` are `!Sync`, while downstream wrappers (`bevy_orzma_term`'s
/// `Component`) need the owning `OrzmaTerm` to be `Send + Sync`.
pub struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    chunk_rx: Receiver<Vec<u8>>,
    exit_rx: Receiver<Option<i32>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Pty {
    /// Opens a PTY at the given grid size, spawns `options.shell` under
    /// it as a login shell, and starts the blocking reader/wait OS
    /// thread.
    pub fn spawn(options: &SpawnOptions) -> OrzmaTermResult<Self> {
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows: options.rows,
                cols: options.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(OrzmaTermError::PtyOpen)?;

        let mut cmd = build_shell_command(&options.shell);
        if let Some(cwd) = options.cwd.as_ref() {
            cmd.cwd(cwd);
        }
        for (key, value) in &options.env {
            cmd.env(&key.0, &value.0);
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(OrzmaTermError::SpawnShell)?;
        let mut child_killer = child.clone_killer();
        drop(pty_pair.slave);

        let (reader, writer) = match master_pipes(pty_pair.master.as_ref()) {
            Ok(pipes) => pipes,
            Err(e) => {
                let _ = child_killer.kill();
                return Err(OrzmaTermError::PtyPipe(e));
            }
        };

        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (exit_tx, exit_rx) = unbounded::<Option<i32>>();
        spawn_reader_thread(reader, child, chunk_tx, exit_tx);

        Ok(Self {
            master: Mutex::new(pty_pair.master),
            writer: Mutex::new(writer),
            chunk_rx,
            exit_rx,
            child_killer,
        })
    }

    #[inline]
    pub fn try_recv_exit(&mut self) -> Option<Option<i32>> {
        self.exit_rx.try_recv().ok()
    }

    #[inline]
    pub fn try_read_chunk(&mut self) -> Option<Vec<u8>> {
        self.chunk_rx.try_recv().ok()
    }

    #[inline]
    pub fn write_all(&mut self, buf: &[u8]) -> OrzmaTermResult {
        self.writer
            .lock()
            .unwrap()
            .write_all(buf)
            .map_err(OrzmaTermError::PtyWrite)?;
        Ok(())
    }

    /// Applies the given grid size to the PTY master (`TIOCSWINSZ`).
    ///
    /// A no-policy wrapper: forwards the values verbatim (validation is
    /// `OrzmaTerm::resize`'s job) with the pixel fields explicitly
    /// zeroed, and maps the master's error to
    /// [`OrzmaTermError::PtyResize`].
    pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTermResult {
        self.master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(OrzmaTermError::PtyResize)
    }

    /// Reads the master's current size back from the kernel
    /// (`TIOCGWINSZ`).
    ///
    /// Panics on ioctl failure — the master fd is no longer valid at
    /// that point (see `OrzmaTerm::pty_size`).
    pub fn size(&self) -> PtySize {
        self.master
            .lock()
            .unwrap()
            .get_size()
            .expect("MasterPty::get_size")
    }

    /// Opens a PTY at the given grid size but routes writes to `writer`
    /// instead of the master, spawning no child process and no reader
    /// thread — the injectable seam behind `OrzmaTerm::detached`.
    pub fn detached(cols: u16, rows: u16, writer: Box<dyn Write + Send>) -> OrzmaTermResult<Self> {
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(OrzmaTermError::PtyOpen)?;
        Ok(Self::with_master(pty_pair.master, writer))
    }

    /// Builds a `Pty` around an arbitrary master and writer, with no
    /// child process and no reader thread — lets tests inject a fake
    /// master (e.g. one whose `resize` fails).
    pub fn with_master(master: Box<dyn MasterPty + Send>, writer: Box<dyn Write + Send>) -> Self {
        let (_, chunk_rx) = unbounded::<Vec<u8>>();
        let (_, exit_rx) = unbounded::<Option<i32>>();
        Self::with_master_and_channels(master, writer, chunk_rx, exit_rx)
    }

    /// Builds a `Pty` like [`Self::with_master`], but with the chunk
    /// and exit streams fed by the given receivers — lets tests inject
    /// PTY output and child-exit reports.
    pub fn with_master_and_channels(
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        chunk_rx: Receiver<Vec<u8>>,
        exit_rx: Receiver<Option<i32>>,
    ) -> Self {
        Self {
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            chunk_rx,
            exit_rx,
            child_killer: Box::new(DetachedKiller),
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child_killer.kill();
    }
}

/// Builds the shell `CommandBuilder`: on macOS the shell is wrapped in
/// `/usr/bin/login` so it runs as a login shell and sources
/// `/etc/zprofile` (`path_helper`) and `~/.zprofile` — without that, an
/// app launched from Finder runs under launchd's minimal `PATH`. Every
/// other platform spawns the shell directly.
fn build_shell_command(shell: &str) -> CommandBuilder {
    #[cfg(target_os = "macos")]
    {
        macos_login_command(shell)
    }
    #[cfg(not(target_os = "macos"))]
    {
        CommandBuilder::new(shell)
    }
}

/// `/usr/bin/login -flp <user> /bin/zsh -fc "exec -a -<name> <shell>"` —
/// runs `shell` as a macOS login shell (so it sources the login
/// profile). Falls back to a direct spawn when `$USER`/`$HOME` are
/// unavailable.
#[cfg(target_os = "macos")]
fn macos_login_command(shell: &str) -> CommandBuilder {
    let user = std::env::var("USER").ok().filter(|s| !s.is_empty());
    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty());
    let (Some(user), Some(home)) = (user, home) else {
        return CommandBuilder::new(shell);
    };
    let shell_name = shell.rsplit('/').next().unwrap_or(shell);
    let exec = format!("exec -a -{shell_name} {shell}");
    // NOTE: the inner shell must be zsh/bash, not sh — `exec -a` (which
    // sets argv0 to `-<name>` to make a login shell) is unavailable in
    // POSIX sh.
    let flags = if PathBuf::from(&home).join(".hushlogin").exists() {
        "-qflp"
    } else {
        "-flp"
    };
    let mut cmd = CommandBuilder::new("/usr/bin/login");
    cmd.args([flags, user.as_str(), "/bin/zsh", "-fc", exec.as_str()]);
    cmd
}

fn master_pipes(
    master: &dyn MasterPty,
) -> anyhow::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    Ok((master.try_clone_reader()?, master.take_writer()?))
}

/// Spawns a dedicated OS thread that drains PTY output into `chunk_tx`
/// and sends a single `exit_tx` message (`Some(code)` on graceful exit,
/// `None` on wait failure) once the reader returns 0 or errors out.
///
/// The OS thread (vs. an async task) is required because the PTY read
/// syscall is blocking.
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if chunk_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_tx.send(code);
    });
}

/// Stand-in child killer for [`Pty::detached`], which has no child
/// process to kill.
#[derive(Debug)]
struct DetachedKiller;

impl ChildKiller for DetachedKiller {
    fn kill(&mut self) -> IoResult<()> {
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(DetachedKiller)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FailingMaster, RecordingMaster};
    use std::io::sink;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Asserts that a resize round-trips through the kernel: the size
    /// read back via `TIOCGWINSZ` is the size just applied.
    ///
    /// Case: the wrapper's one real side effect. The non-square 120x40
    /// pins the argument-to-field mapping — the method takes
    /// `(cols, rows)` while `PtySize` declares `rows` first, so a
    /// transposition compiles silently and swaps every grid dimension.
    #[test]
    fn resize_applies_the_size_to_the_kernel() {
        let mut pty = Pty::detached(80, 24, Box::new(sink())).expect("Pty::detached");
        pty.resize(120, 40).expect("resize");
        let size = pty.size();
        assert_eq!((size.cols, size.rows), (120, 40));
    }

    /// Asserts the exact `PtySize` handed to the master: the requested
    /// cols/rows in the right fields and both pixel fields zero.
    ///
    /// Case: the kernel-readback test cannot see this — a detached
    /// master already starts at pixel 0, so "explicitly wrote 0" and
    /// "preserved the old value" are indistinguishable there. Recording
    /// the forwarded struct pins the decided pixel policy (explicitly
    /// zeroed, consistent with `spawn` / `detached`).
    #[test]
    fn resize_forwards_the_exact_pty_size_to_the_master() {
        let (master, calls) = RecordingMaster::new();
        let mut pty = Pty::with_master(Box::new(master), Box::new(sink()));
        pty.resize(120, 40).expect("resize");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0
            }]
        );
    }

    /// Asserts that degenerate sizes are forwarded verbatim, one master
    /// call per request.
    ///
    /// Case: the layering pin. The zero-axis / oversize policy lives
    /// only in `OrzmaTerm::resize`; this wrapper must not validate. A
    /// guard sneaking in here would duplicate the policy and let the
    /// two layers drift (one clamping while the other ignores) without
    /// any layered test noticing.
    #[test]
    fn resize_forwards_degenerate_sizes_verbatim() {
        let (master, calls) = RecordingMaster::new();
        let mut pty = Pty::with_master(Box::new(master), Box::new(sink()));
        for (cols, rows) in [(0, 0), (0, 40), (120, 0)] {
            pty.resize(cols, rows).expect("resize");
        }
        let recorded: Vec<(u16, u16)> = calls
            .lock()
            .unwrap()
            .iter()
            .map(|s| (s.cols, s.rows))
            .collect();
        assert_eq!(recorded, vec![(0, 0), (0, 40), (120, 0)]);
    }

    /// Asserts that a master resize failure surfaces as
    /// `OrzmaTermError::PtyResize`.
    ///
    /// Case: the error-taxonomy pin, mirroring `write_all` →
    /// `PtyWrite`. `OrzmaTerm::resize`'s failure-atomicity branch and
    /// the bevy layer's `error!` log both identify the failing
    /// subsystem by this variant.
    #[test]
    fn a_failing_master_maps_to_pty_resize_error() {
        let mut pty = Pty::with_master(Box::new(FailingMaster), Box::new(sink()));
        let result = pty.resize(120, 40);
        assert!(
            matches!(result, Err(OrzmaTermError::PtyResize(_))),
            "expected PtyResize, got {result:?}"
        );
    }

    /// Asserts that the child's exit is reported exactly once: the
    /// first successful read yields the exit code and every later call
    /// yields `None`.
    ///
    /// Case: the shell process exits while the host keeps polling every
    /// frame for output and exit state.
    #[test]
    fn exit_is_reported_once_after_the_child_terminates() {
        let mut pty = Pty::spawn(&SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/echo".into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("Pty::spawn failed");
        let deadline = Instant::now() + Duration::from_secs(10);
        let code = loop {
            if let Some(code) = pty.try_recv_exit() {
                break code;
            }
            assert!(Instant::now() < deadline, "no exit report arrived");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(code, Some(0));
        for _ in 0..3 {
            assert_eq!(
                pty.try_recv_exit(),
                None,
                "the exit must not be re-reported"
            );
        }
    }

    /// Asserts that a PTY without a child process never reports an
    /// exit.
    ///
    /// Case: a detached test terminal (or one built around an injected
    /// master) is polled for an exit the same way a live terminal is.
    #[test]
    fn a_detached_pty_never_reports_an_exit() {
        let mut detached = Pty::detached(80, 24, Box::new(sink())).expect("Pty::detached");
        let mut injected = Pty::with_master(Box::new(FailingMaster), Box::new(sink()));
        for _ in 0..3 {
            assert_eq!(detached.try_recv_exit(), None);
            assert_eq!(injected.try_recv_exit(), None);
        }
    }

    #[test]
    fn spawn_emits_chunk_and_exit_zero() {
        let pty = Pty::spawn(&SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/echo".into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("Pty::spawn failed");
        let chunk = pty
            .chunk_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no PTY chunk arrived");
        assert!(!chunk.is_empty());
        let code = pty
            .exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no exit code arrived");
        assert_eq!(code, Some(0));
    }
}
