//! `Pty` — owns the PTY master, writer, child killer, and the

use crate::{
    CellPixels, SpawnOptions,
    error::{OrzmaTtyError, OrzmaTtyResult},
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
#[cfg(any(test, feature = "test-support"))]
use std::io::Result as IoResult;
use std::io::{Read, Write};
#[cfg(any(test, feature = "test-support"))]
use std::mem;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

/// PTY ownership for one spawned shell.
///
/// `Mutex` is required because `dyn MasterPty + Send` and `dyn Write +
/// Send` are `!Sync`, while downstream wrappers (`bevy_orzma_mux`'s
/// `Component`) need the owning `OrzmaTty` to be `Send + Sync`.
pub struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    chunk_rx: Receiver<Vec<u8>>,
    exit_rx: Receiver<Option<i32>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
}

/// One non-blocking read of the PTY output stream.
#[derive(Debug, PartialEq, Eq)]
pub enum ChunkPoll {
    /// A chunk of PTY output.
    Chunk(Vec<u8>),
    /// Nothing queued right now; the reader thread is still alive.
    Empty,
    /// The reader thread is gone and nothing remains queued.
    Disconnected,
}

/// One non-blocking read of the child-exit stream.
#[derive(Debug, PartialEq, Eq)]
pub enum ExitPoll {
    /// The child exited; `None` if the `wait` itself failed.
    Exited(Option<i32>),
    /// The child is still running (or its status is not yet sent).
    Pending,
    /// The reader thread is gone without ever sending a status.
    Disconnected,
}

impl Pty {
    /// Opens a PTY at the given grid size, spawns `options.shell` under
    /// it as a login shell, and starts the blocking reader/wait OS
    /// thread.
    ///
    /// On Windows, ConPTY writes `CSI 6 n` before it starts the child and
    /// holds the child until a cursor-position report arrives; the owner
    /// must write that reply through [`Self::write_all`]. `OrzmaTty` does
    /// so with the VT's replies (see `Screen::cursor_position_report`).
    pub fn spawn(options: &SpawnOptions) -> OrzmaTtyResult<Self> {
        let (pixel_width, pixel_height) = options.cell_px.window_pixels(options.cols, options.rows);
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows: options.rows,
                cols: options.cols,
                pixel_width,
                pixel_height,
            })
            .map_err(OrzmaTtyError::PtyOpen)?;

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
            .map_err(OrzmaTtyError::SpawnShell)?;
        let mut child_killer = child.clone_killer();
        drop(pty_pair.slave);

        let (reader, writer) = match master_pipes(pty_pair.master.as_ref()) {
            Ok(pipes) => pipes,
            Err(e) => {
                let _ = child_killer.kill();
                return Err(OrzmaTtyError::PtyPipe(e));
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

    /// Polls the output stream once (see [`ChunkPoll`]).
    #[inline]
    pub fn poll_chunk(&self) -> ChunkPoll {
        match self.chunk_rx.try_recv() {
            Ok(chunk) => ChunkPoll::Chunk(chunk),
            Err(TryRecvError::Empty) => ChunkPoll::Empty,
            Err(TryRecvError::Disconnected) => ChunkPoll::Disconnected,
        }
    }

    /// Polls the exit stream once (see [`ExitPoll`]).
    #[inline]
    pub fn poll_exit(&self) -> ExitPoll {
        match self.exit_rx.try_recv() {
            Ok(code) => ExitPoll::Exited(code),
            Err(TryRecvError::Empty) => ExitPoll::Pending,
            Err(TryRecvError::Disconnected) => ExitPoll::Disconnected,
        }
    }

    /// Whether output chunks are queued and unread.
    #[inline]
    pub fn chunks_pending(&self) -> bool {
        !self.chunk_rx.is_empty()
    }

    /// The output stream, for a `Select` that waits on many terminals.
    #[inline]
    pub fn chunk_receiver(&self) -> &Receiver<Vec<u8>> {
        &self.chunk_rx
    }

    /// The exit stream, for a `Select` that waits on many terminals.
    #[inline]
    pub fn exit_receiver(&self) -> &Receiver<Option<i32>> {
        &self.exit_rx
    }

    #[inline]
    pub fn write_all(&mut self, buf: &[u8]) -> OrzmaTtyResult {
        self.writer
            .lock()
            .unwrap()
            .write_all(buf)
            .map_err(OrzmaTtyError::PtyWrite)?;
        Ok(())
    }

    /// Applies the given grid size to the PTY master (`TIOCSWINSZ`),
    /// with the pixel fields set to the total window pixels
    /// `cell_px × cells` (see [`CellPixels::window_pixels`]).
    ///
    /// A no-policy wrapper: forwards the cell counts verbatim (validation
    /// is `OrzmaTty::resize`'s job) and maps the master's error to
    /// [`OrzmaTtyError::PtyResize`].
    pub fn resize(&mut self, cols: u16, rows: u16, cell_px: CellPixels) -> OrzmaTtyResult {
        let (pixel_width, pixel_height) = cell_px.window_pixels(cols, rows);
        self.master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width,
                pixel_height,
            })
            .map_err(OrzmaTtyError::PtyResize)
    }

    /// Reads the master's current size back from the kernel
    /// (`TIOCGWINSZ`).
    ///
    /// Panics on ioctl failure — the master fd is no longer valid at
    /// that point (see `OrzmaTty::pty_size`).
    pub fn size(&self) -> PtySize {
        self.master
            .lock()
            .unwrap()
            .get_size()
            .expect("MasterPty::get_size")
    }

    /// Opens a real PTY at the given grid size but routes writes to
    /// `writer` instead of the master, spawning no child process and no
    /// reader thread.
    ///
    /// `orzma_tty`'s own `#[cfg(test)]` tests are the only remaining
    /// caller — `OrzmaTty::detached` builds around
    /// [`crate::test_support::RecordingMaster`] instead so that
    /// downstream test fixtures never open a real PTY.
    #[cfg(test)]
    pub fn detached(cols: u16, rows: u16, writer: Box<dyn Write + Send>) -> OrzmaTtyResult<Self> {
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(OrzmaTtyError::PtyOpen)?;
        Ok(Self::with_master(pty_pair.master, writer))
    }

    /// Builds a `Pty` around an arbitrary master and writer, with no
    /// child process and no reader thread — lets tests inject a fake
    /// master (e.g. one whose `resize` fails).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_master(master: Box<dyn MasterPty + Send>, writer: Box<dyn Write + Send>) -> Self {
        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (exit_tx, exit_rx) = unbounded::<Option<i32>>();
        // NOTE: the senders are leaked, not dropped: a disconnected
        // receiver reads as "the reader thread is gone" to
        // `OrzmaTty::pump`, which would synthesize a spurious
        // `ChildExit` for a fixture that never had a child process at
        // all. Leaking keeps both streams `Pending` forever instead.
        mem::forget(chunk_tx);
        mem::forget(exit_tx);
        Self::with_master_and_channels(master, writer, chunk_rx, exit_rx)
    }

    /// Builds a `Pty` like [`Self::with_master`], but with the chunk
    /// and exit streams fed by the given receivers — lets tests inject
    /// PTY output and child-exit reports.
    #[cfg(any(test, feature = "test-support"))]
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
#[cfg(unix)]
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
        // NOTE: the exit status must be sent before this closure returns
        // and drops `chunk_tx` / `exit_tx`: `OrzmaTty::pump` treats a
        // disconnected chunk stream as fully drained, and the backend's
        // `Select` treats a disconnected receiver as permanently ready, so
        // dropping the senders first would leave the pane unclosable and
        // spinning until the synthesized `ChildExit` fallback fires.
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_tx.send(code);
    });
}

/// Spawns the reader thread that drains PTY output into `chunk_tx`, and a
/// second thread that waits for the child and sends the single `exit_tx`
/// message once the output has gone quiet.
///
/// ConPTY keeps the output pipe open after the child exits until the
/// pseudoconsole is closed, so the reader cannot learn about the exit from
/// EOF the way the Unix reader does. The watcher waits [`OUTPUT_QUIESCENCE`]
/// after the reader's last completed read so the child's final output is
/// queued before the exit is reported; `OrzmaTty::pump` then reports
/// `ChildExit` only once the queue is drained. The reader ends when the
/// master is dropped by the pane teardown the exit triggers.
#[cfg(windows)]
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
) {
    let last_read = Arc::new(Mutex::new(Instant::now()));
    let reader_clock = Arc::clone(&last_read);
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    *reader_clock.lock().unwrap() = Instant::now();
                    if chunk_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    thread::spawn(move || {
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        wait_for_output_quiescence(&last_read);
        let _ = exit_tx.send(code);
    });
}

/// How long the output stream must stay idle after the child exits
/// before the exit is reported.
#[cfg(windows)]
const OUTPUT_QUIESCENCE: Duration = Duration::from_millis(50);

/// The longest the watcher waits for the output to go quiet, so a
/// pseudoconsole that keeps streaming cannot delay the exit forever.
#[cfg(windows)]
const OUTPUT_QUIESCENCE_CAP: Duration = Duration::from_secs(2);

/// Blocks until no read has completed for [`OUTPUT_QUIESCENCE`], or
/// [`OUTPUT_QUIESCENCE_CAP`] has passed.
#[cfg(windows)]
fn wait_for_output_quiescence(last_read: &Mutex<Instant>) {
    let cap = Instant::now() + OUTPUT_QUIESCENCE_CAP;
    loop {
        let idle = last_read.lock().unwrap().elapsed();
        if idle >= OUTPUT_QUIESCENCE || Instant::now() >= cap {
            return;
        }
        thread::sleep(OUTPUT_QUIESCENCE - idle);
    }
}

/// Stand-in child killer for the PTY-less constructors, which have no
/// child process to kill.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
struct DetachedKiller;

#[cfg(any(test, feature = "test-support"))]
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

    /// A program that prints one line and exits 0 on the host platform.
    fn echo_program() -> &'static str {
        if cfg!(windows) { "whoami" } else { "/bin/echo" }
    }

    /// Answers the cursor-position query ConPTY writes before it starts
    /// the child. A `Pty` driven without a VT must reply itself.
    #[cfg(windows)]
    fn answer_cursor_query(pty: &mut Pty) {
        pty.write_all(b"\x1b[1;1R").expect("reply to CSI 6 n");
    }

    #[cfg(unix)]
    fn answer_cursor_query(_pty: &mut Pty) {}

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
        pty.resize(120, 40, CellPixels::default()).expect("resize");
        let size = pty.size();
        assert_eq!((size.cols, size.rows), (120, 40));
    }

    /// Asserts that a resize writes `cell_px × cells` into the winsize
    /// pixel fields, not the per-cell pitch.
    ///
    /// Case: the host reports an 8×16 px cell and resizes the pane to
    /// 100×40; a program reading `TIOCGWINSZ` must see 800×640.
    #[test]
    fn resize_writes_the_total_window_pixels() {
        let (master, calls) = RecordingMaster::at(80, 24);
        let mut pty = Pty::with_master(Box::new(master), Box::new(sink()));
        pty.resize(
            100,
            40,
            CellPixels {
                width: 8,
                height: 16,
            },
        )
        .expect("resize");
        let last = *calls.lock().unwrap().last().expect("one resize call");
        assert_eq!((last.cols, last.rows), (100, 40));
        assert_eq!((last.pixel_width, last.pixel_height), (800, 640));
    }

    /// Asserts that degenerate sizes are forwarded verbatim, one master
    /// call per request.
    ///
    /// Case: the layering pin. The zero-axis / oversize policy lives
    /// only in `OrzmaTty::resize`; this wrapper must not validate. A
    /// guard sneaking in here would duplicate the policy and let the
    /// two layers drift (one clamping while the other ignores) without
    /// any layered test noticing.
    #[test]
    fn resize_forwards_degenerate_sizes_verbatim() {
        let (master, calls) = RecordingMaster::at(80, 24);
        let mut pty = Pty::with_master(Box::new(master), Box::new(sink()));
        for (cols, rows) in [(0, 0), (0, 40), (120, 0)] {
            pty.resize(cols, rows, CellPixels::default())
                .expect("resize");
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
    /// `OrzmaTtyError::PtyResize`.
    ///
    /// Case: the error-taxonomy pin, mirroring `write_all` →
    /// `PtyWrite`. `OrzmaTty::resize`'s failure-atomicity branch and
    /// the bevy layer's `error!` log both identify the failing
    /// subsystem by this variant.
    #[test]
    fn a_failing_master_maps_to_pty_resize_error() {
        let mut pty = Pty::with_master(Box::new(FailingMaster), Box::new(sink()));
        let result = pty.resize(120, 40, CellPixels::default());
        assert!(
            matches!(result, Err(OrzmaTtyError::PtyResize(_))),
            "expected PtyResize, got {result:?}"
        );
    }

    /// Asserts that the child's exit is reported exactly once: the
    /// first successful poll yields the exit code, and every later poll
    /// finds the reader thread gone and reports `Disconnected` rather
    /// than replaying the code.
    ///
    /// Case: the shell process exits while the host keeps polling every
    /// frame for output and exit state.
    #[test]
    fn exit_is_reported_once_after_the_child_terminates() {
        let mut pty = Pty::spawn(&SpawnOptions {
            cols: 80,
            rows: 24,
            cell_px: CellPixels::default(),
            shell: echo_program().into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("Pty::spawn failed");
        answer_cursor_query(&mut pty);
        let deadline = Instant::now() + Duration::from_secs(10);
        let code = loop {
            if let ExitPoll::Exited(code) = pty.poll_exit() {
                break code;
            }
            assert!(Instant::now() < deadline, "no exit report arrived");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(code, Some(0));
        for _ in 0..3 {
            assert_eq!(
                pty.poll_exit(),
                ExitPoll::Disconnected,
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
        let detached = Pty::detached(80, 24, Box::new(sink())).expect("Pty::detached");
        let injected = Pty::with_master(Box::new(FailingMaster), Box::new(sink()));
        for _ in 0..3 {
            assert_eq!(detached.poll_exit(), ExitPoll::Pending);
            assert_eq!(injected.poll_exit(), ExitPoll::Pending);
        }
    }

    #[test]
    fn spawn_emits_chunk_and_exit_zero() {
        let mut pty = Pty::spawn(&SpawnOptions {
            cols: 80,
            rows: 24,
            cell_px: CellPixels::default(),
            shell: echo_program().into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("Pty::spawn failed");
        answer_cursor_query(&mut pty);
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

    /// Asserts that the child's last output is delivered before its
    /// exit is reported.
    ///
    /// Case: a Windows shell prints a farewell line and exits; the pane
    /// must show the line before it closes.
    #[cfg(windows)]
    #[test]
    fn the_final_output_precedes_the_exit_report() {
        let mut pty = Pty::spawn(&SpawnOptions {
            cols: 80,
            rows: 24,
            cell_px: CellPixels::default(),
            shell: echo_program().into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("Pty::spawn failed");
        answer_cursor_query(&mut pty);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_output = false;
        loop {
            if let ChunkPoll::Chunk(chunk) = pty.poll_chunk() {
                saw_output |= !chunk.is_empty();
            }
            if let ExitPoll::Exited(code) = pty.poll_exit() {
                assert_eq!(code, Some(0));
                assert!(
                    saw_output,
                    "the exit was reported before any output arrived"
                );
                break;
            }
            assert!(Instant::now() < deadline, "no exit report arrived");
            thread::sleep(Duration::from_millis(1));
        }
    }
}
