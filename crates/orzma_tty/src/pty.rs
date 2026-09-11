//! `Pty` — owns the PTY master, writer, child killer, and the output and
//! exit streams its OS threads feed.

use crate::{
    CellPixels, SpawnOptions,
    error::{OrzmaTtyError, OrzmaTtyResult},
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded, unbounded};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
#[cfg(any(test, feature = "test-support"))]
use std::io::Result as IoResult;
use std::io::{Read, Write};
#[cfg(any(test, feature = "test-support"))]
use std::mem;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
#[cfg(any(windows, test))]
use std::time::Duration;
use std::time::Instant;

/// PTY ownership for one spawned shell.
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
    /// No status is queued while the thread that sends it is still alive.
    ///
    /// Besides a running child, this covers the moment after the status
    /// has been received but before the sending thread has exited.
    Pending,
    /// The thread that sends the status is gone and no status remains
    /// queued.
    Disconnected,
}

impl Pty {
    /// How many reader chunks may wait unparsed before the reader thread
    /// parks: 256 × 4 KiB = 1 MiB per pane.
    pub const CHUNK_QUEUE_CAPACITY: usize = 256;

    /// Opens a PTY at the given grid size, spawns `options.shell` under
    /// it (as a login shell on macOS), and starts the blocking OS thread
    /// (two on Windows) that reads its output and waits for the child.
    ///
    /// On Windows, ConPTY writes `CSI 6 n` before it starts the child and
    /// holds the child until a cursor-position report arrives; the owner
    /// must write that reply through [`Self::write_all`].
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

        let (chunk_tx, chunk_rx) = bounded::<Vec<u8>>(Self::CHUNK_QUEUE_CAPACITY);
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

    /// The PTY output stream.
    #[inline]
    pub fn chunk_receiver(&self) -> &Receiver<Vec<u8>> {
        &self.chunk_rx
    }

    /// The child-exit stream.
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
    /// Forwards the cell counts verbatim, without validating them, and
    /// maps the master's error to [`OrzmaTtyError::PtyResize`].
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
    /// Panics on ioctl failure, which means the master fd is no longer
    /// valid.
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
    /// child process and no reader thread.
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
    /// and exit streams fed by the given receivers, so the caller
    /// supplies the PTY output and the child-exit reports.
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
/// `/etc/zprofile` (`path_helper`) and `~/.zprofile`. Every other
/// platform spawns the shell directly.
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

/// What the reader thread reports about its progress.
struct ReaderProgress {
    /// When the reader last completed a read or a send.
    last_activity: Mutex<Instant>,
    /// `true` from the moment `try_send` finds the queue full until the
    /// blocking `send` returns.
    parked: AtomicBool,
}

impl ReaderProgress {
    /// Progress for a reader that has just started: not parked, active
    /// at `now`.
    fn new(now: Instant) -> Self {
        Self {
            last_activity: Mutex::new(now),
            parked: AtomicBool::new(false),
        }
    }

    /// Whether a `send` is waiting on a full queue right now.
    #[cfg(any(windows, test))]
    fn is_parked(&self) -> bool {
        self.parked.load(Ordering::Acquire)
    }

    /// How long before `now` the reader last completed a read or a send.
    #[cfg(any(windows, test))]
    fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(*self.last_activity.lock().unwrap())
    }

    fn stamp(&self, now: Instant) {
        *self.last_activity.lock().unwrap() = now;
    }
}

/// Reads `reader` to EOF or error, sending each read as one chunk.
/// Returns when the reader ends or the receiver is gone.
///
/// `progress` is stamped after every completed read and again after a
/// blocking `send` returns, and its parked flag holds from the moment
/// `try_send` finds the queue full until the blocking `send` returns.
///
/// # Invariants
///
/// The stamp after a blocking `send` is written before the parked flag
/// clears, so a watcher that observes the flag clear also sees the fresh
/// stamp.
fn forward_chunks(reader: &mut dyn Read, chunk_tx: &Sender<Vec<u8>>, progress: &ReaderProgress) {
    let mut buf = [0u8; 4096];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        progress.stamp(Instant::now());
        let delivered = match chunk_tx.try_send(buf[..n].to_vec()) {
            Ok(()) => true,
            Err(TrySendError::Full(chunk)) => {
                progress.parked.store(true, Ordering::Release);
                let delivered = chunk_tx.send(chunk).is_ok();
                progress.stamp(Instant::now());
                progress.parked.store(false, Ordering::Release);
                delivered
            }
            Err(TrySendError::Disconnected(_)) => false,
        };
        if !delivered {
            return;
        }
    }
}

/// Spawns a dedicated OS thread that drains PTY output into `chunk_tx`
/// and sends a single `exit_tx` message (`Some(code)` on graceful exit,
/// `None` on wait failure) once the reader returns 0 or errors out.
#[cfg(unix)]
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
) {
    let progress = Arc::new(ReaderProgress::new(Instant::now()));
    thread::spawn(move || {
        forward_chunks(reader.as_mut(), &chunk_tx, &progress);
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
/// The watcher waits [`OUTPUT_QUIESCENCE`] after the reader's last completed
/// read or send, and only while the reader is not parked on a full queue,
/// so the child's final output is queued before the exit is reported.
/// The reader sees no EOF at the child's exit and ends when the master is
/// dropped or a send finds the receiver gone.
#[cfg(windows)]
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
) {
    let progress = Arc::new(ReaderProgress::new(Instant::now()));
    let reader_progress = Arc::clone(&progress);
    thread::spawn(move || forward_chunks(reader.as_mut(), &chunk_tx, &reader_progress));
    thread::spawn(move || {
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        wait_for_output_quiescence(&progress, OUTPUT_QUIESCENCE, OUTPUT_QUIESCENCE_CAP);
        let _ = exit_tx.send(code);
    });
}

/// How long the output stream must stay idle after the child exits
/// before the exit is reported.
#[cfg(windows)]
const OUTPUT_QUIESCENCE: Duration = Duration::from_millis(50);

/// The longest the watcher lets an unparked reader stream after the
/// child exits.
#[cfg(windows)]
const OUTPUT_QUIESCENCE_CAP: Duration = Duration::from_secs(2);

/// The shortest sleep between two polls of the reader's progress.
#[cfg(any(windows, test))]
const QUIESCENCE_POLL_FLOOR: Duration = Duration::from_millis(10);

/// Blocks until the reader is not parked and no read or send has
/// completed for `quiescence`, or until the reader has spent `cap` in
/// total not parked.
///
/// Time spent parked counts toward neither the idle window nor the cap,
/// so the call does not return while the child's output still waits in
/// the pipe behind a full queue. An unpark restarts the idle window from
/// the send stamp.
///
/// The unparked total is measured per poll interval: a park that spans
/// a poll is never charged to the cap, while one that fits inside a
/// single interval still is.
#[cfg(any(windows, test))]
fn wait_for_output_quiescence(progress: &ReaderProgress, quiescence: Duration, cap: Duration) {
    let mut unparked = Duration::ZERO;
    let mut last_poll = Instant::now();
    let mut was_parked = progress.is_parked();
    loop {
        let now = Instant::now();
        let parked = progress.is_parked();
        if !parked && !was_parked {
            unparked += now.saturating_duration_since(last_poll);
        }
        was_parked = parked;
        last_poll = now;
        let idle = progress.idle_for(now);
        if (!parked && idle >= quiescence) || unparked >= cap {
            return;
        }
        let wait = if parked {
            quiescence
        } else {
            quiescence.saturating_sub(idle)
        };
        thread::sleep(wait.max(QUIESCENCE_POLL_FLOOR));
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
    use crossbeam_channel::bounded;
    use std::io::Cursor;
    use std::io::sink;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

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
    /// Case: the host applies a non-square 120x40 geometry to a pane's
    /// PTY, and a program reads the size back with `TIOCGWINSZ`.
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
    /// 100×40 while a program reads `TIOCGWINSZ`.
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

    /// Asserts that degenerate sizes are forwarded verbatim rather than
    /// validated, one master call per request.
    ///
    /// Case: a caller hands the PTY a zero-axis geometry, such as the one
    /// a minimized window computes.
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
    /// Case: the kernel refuses the winsize ioctl when the host resizes a
    /// pane.
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
    /// first successful poll yields the exit code, and later polls report
    /// `Pending` until the sending thread is gone and then `Disconnected`,
    /// never replaying the code.
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
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match pty.poll_exit() {
                ExitPoll::Disconnected => break,
                ExitPoll::Pending => {
                    assert!(
                        Instant::now() < deadline,
                        "the exit stream never disconnected"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                ExitPoll::Exited(code) => panic!("the exit was re-reported with {code:?}"),
            }
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
    /// Case: a Windows shell prints a farewell line and exits.
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

    /// Spawns `forward_chunks` over `input` on its own thread and returns
    /// the receiver, the shared progress, and the thread handle.
    fn forwarding(
        input: Vec<u8>,
        capacity: usize,
    ) -> (Receiver<Vec<u8>>, Arc<ReaderProgress>, JoinHandle<()>) {
        let (chunk_tx, chunk_rx) = bounded::<Vec<u8>>(capacity);
        let progress = Arc::new(ReaderProgress::new(Instant::now()));
        let thread_progress = Arc::clone(&progress);
        let reader = thread::spawn(move || {
            let mut cursor = Cursor::new(input);
            forward_chunks(&mut cursor, &chunk_tx, &thread_progress);
        });
        (chunk_rx, progress, reader)
    }

    /// Polls `condition` every millisecond and reports whether it held
    /// before `within` elapsed.
    fn holds_within(mut condition: impl FnMut() -> bool, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while !condition() {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    /// Whether `handle` finishes within `within`.
    fn finishes_within(handle: &JoinHandle<()>, within: Duration) -> bool {
        holds_within(|| handle.is_finished(), within)
    }

    /// Blocks until `progress` reports the reader parked, failing after
    /// ten seconds.
    fn wait_until_parked(progress: &ReaderProgress) {
        assert!(
            holds_within(|| progress.is_parked(), Duration::from_secs(10)),
            "the reader never parked"
        );
    }

    /// Asserts that the reader parks once the queue holds
    /// `CHUNK_QUEUE_CAPACITY` chunks and forwards every byte once the
    /// queue drains.
    ///
    /// Case: a child writes 2 MiB faster than the parser reads it, and
    /// the parser catches up later.
    #[test]
    fn the_reader_parks_at_the_capacity_and_forwards_every_byte() {
        let input = vec![0xABu8; 2 * 1024 * 1024];
        let expected = input.len();
        let (chunk_rx, progress, reader) = forwarding(input, Pty::CHUNK_QUEUE_CAPACITY);
        wait_until_parked(&progress);
        assert_eq!(chunk_rx.len(), Pty::CHUNK_QUEUE_CAPACITY);
        let forwarded: usize = chunk_rx.iter().map(|chunk| chunk.len()).sum();
        reader.join().expect("the reader thread ends at EOF");
        assert_eq!(forwarded, expected);
    }

    /// Asserts that dropping the receiver while the reader is parked ends
    /// the reader thread.
    ///
    /// Case: the user kills a pane whose parser fell behind a flooding
    /// child, so the queue is full when the pane is torn down.
    #[test]
    fn dropping_the_receiver_while_parked_ends_the_reader() {
        let (chunk_rx, progress, reader) =
            forwarding(vec![0u8; 2 * 1024 * 1024], Pty::CHUNK_QUEUE_CAPACITY);
        wait_until_parked(&progress);
        drop(chunk_rx);
        assert!(
            finishes_within(&reader, Duration::from_secs(10)),
            "the parked reader did not end"
        );
        reader
            .join()
            .expect("the reader thread ends on a gone receiver");
    }

    /// Runs the watcher on its own thread with the given windows.
    fn watch(
        progress: &Arc<ReaderProgress>,
        quiescence: Duration,
        cap: Duration,
    ) -> JoinHandle<()> {
        let progress = Arc::clone(progress);
        thread::spawn(move || wait_for_output_quiescence(&progress, quiescence, cap))
    }

    /// Asserts that the watcher does not return while the reader is
    /// parked, even after the idle window elapsed and even past the cap.
    ///
    /// Case: the child exits with its last output still in the pipe
    /// behind a full queue, and the parser takes longer than the cap to
    /// catch up.
    #[test]
    fn the_watcher_waits_while_the_reader_is_parked_even_past_the_cap() {
        let progress = Arc::new(ReaderProgress::new(Instant::now()));
        progress.parked.store(true, Ordering::Release);
        let watcher = watch(
            &progress,
            Duration::from_millis(20),
            Duration::from_millis(100),
        );
        assert!(
            !finishes_within(&watcher, Duration::from_millis(300)),
            "the watcher returned while the reader was parked"
        );
        progress.stamp(Instant::now());
        progress.parked.store(false, Ordering::Release);
        assert!(finishes_within(&watcher, Duration::from_secs(2)));
    }

    /// Asserts that after an unpark the watcher waits a fresh idle window
    /// from the send stamp, even though the read stamp is already older
    /// than the window.
    ///
    /// Case: a send parked longer than the idle window returns, and the
    /// child's next read has not completed yet.
    #[test]
    fn an_unpark_restarts_the_idle_window_from_the_send_stamp() {
        let quiescence = Duration::from_millis(50);
        let stale = Instant::now()
            .checked_sub(Duration::from_millis(200))
            .expect("a monotonic instant 200 ms ago exists");
        let progress = Arc::new(ReaderProgress::new(stale));
        progress.parked.store(true, Ordering::Release);
        let watcher = watch(&progress, quiescence, Duration::from_secs(2));
        thread::sleep(Duration::from_millis(100));
        let unparked_at = Instant::now();
        progress.stamp(unparked_at);
        progress.parked.store(false, Ordering::Release);
        assert!(finishes_within(&watcher, Duration::from_secs(2)));
        assert!(
            unparked_at.elapsed() >= quiescence,
            "the watcher returned before a fresh idle window elapsed"
        );
    }

    /// Asserts that the cap still bounds an unparked reader that never
    /// goes idle.
    ///
    /// Case: a pseudoconsole keeps streaming after the child exited and
    /// the reader keeps completing reads.
    #[test]
    fn the_cap_bounds_an_unparked_reader_that_never_goes_idle() {
        let progress = Arc::new(ReaderProgress::new(Instant::now()));
        let stop = Arc::new(AtomicBool::new(false));
        let stamper = {
            let progress = Arc::clone(&progress);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    progress.stamp(Instant::now());
                    thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let watcher = watch(
            &progress,
            Duration::from_millis(50),
            Duration::from_millis(150),
        );
        let finished = finishes_within(&watcher, Duration::from_secs(2));
        stop.store(true, Ordering::Release);
        stamper.join().expect("the stamper ends");
        assert!(finished);
    }

    /// Asserts that an idle, unparked reader lets the watcher return
    /// after one idle window.
    ///
    /// Case: the child printed its farewell, the reader queued it, and
    /// nothing else arrives.
    #[test]
    fn an_idle_unparked_reader_returns_after_one_window() {
        let started = Instant::now();
        let progress = Arc::new(ReaderProgress::new(started));
        let watcher = watch(&progress, Duration::from_millis(30), Duration::from_secs(2));
        assert!(finishes_within(&watcher, Duration::from_secs(2)));
        watcher.join().expect("the watcher ends");
        assert!(started.elapsed() >= Duration::from_millis(30));
    }
}
