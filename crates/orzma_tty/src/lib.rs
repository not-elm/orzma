//! PTY-backed terminal core: spawns a shell under a PTY and
//! drives an injected [`Vt`] implementor behind a frame coalescer.

use crate::{
    coalescer::Coalescer,
    error::OrzmaTtyResult,
    input::{
        MouseReport, MouseReportKind, PtyInput, TerminalKey, TerminalModifiers, WheelConfig,
        WheelDecision, WheelInput,
    },
    pty::{ChunkPoll, ExitPoll, Pty},
    signal::TtySignal,
};
use crossbeam_channel::Receiver;
use orzma_vt::prelude::*;
use portable_pty::PtySize;
#[cfg(any(test, feature = "test-support"))]
use std::io::Write;
use std::path::PathBuf;
use std::{mem, time::Instant};
#[cfg(any(test, feature = "test-support"))]
use test_support::RecordingMaster;

mod cell_pixels;
mod coalescer;
mod error;
mod input;
mod pty;
mod signal;
pub mod test_support;

pub use cell_pixels::CellPixels;

pub mod prelude {
    pub use crate::{CellPixels, OrzmaTty, PumpOutput, Readiness, error::*, input::*, signal::*};
}

/// Spawn parameters consumed exactly once by `OrzmaTty::spawn`.
pub struct SpawnOptions {
    /// Terminal grid size.
    pub size: GridSize,
    /// Physical pixels per cell, projected onto the PTY winsize.
    pub cell_px: CellPixels,
    /// Shell program to launch (absolute path or `$PATH`-resolvable name).
    pub shell: String,
    /// Initial working directory for the spawned shell.
    pub cwd: Option<PathBuf>,
    /// Arbitrary environment variables forwarded to the shell.
    pub env: Vec<(EnvKey, EnvValue)>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvKey(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvValue(pub String);

/// Everything one [`OrzmaTty::pump`] call produced.
///
/// One pump folds several interpreted chunks into at most one frame plus
/// the signals they raised.
pub struct PumpOutput {
    /// The frame to draw, present only when the coalesce window came due or
    /// the bootstrap snapshot was still owed.
    pub frame: Option<Frame>,
    /// Signals raised since the previous pump, in order, with
    /// `ChildExit` last.
    pub signals: Vec<TtySignal>,
    /// Whether output chunks remain queued after this pump's budget was
    /// spent, so the owner should pump again before waiting.
    pub more_pending: bool,
}

/// The receivers to wait on to learn when a terminal has work: its output
/// stream, and its exit stream until the exit has been observed.
pub struct Readiness<'a> {
    /// The PTY output stream.
    pub chunks: &'a Receiver<Vec<u8>>,
    /// The child-exit stream, or `None` once [`OrzmaTty::pump`] latched
    /// the exit, at which point it must leave the wait set.
    pub exit: Option<&'a Receiver<Option<i32>>>,
}

/// Where a terminal stands in reporting its child's exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitLatch {
    /// The child is running as far as the terminal knows.
    Running,
    /// The exit status was observed; `ChildExit` is owed once the output
    /// stream drains.
    Observed(Option<i32>),
    /// `ChildExit` was reported; nothing more to say.
    Reported,
}

/// A live terminal: the VT emulation plus the PTY it is wired to.
pub struct OrzmaTty<V: Vt> {
    vt: V,
    coalescer: Coalescer,
    pty: Pty,
    /// Signals produced by interpreted chunks or reported by a resize,
    /// awaiting the next pump.
    pending_signals: Vec<TtySignal>,
    /// Reply bytes produced by interpreted chunks, which the next pump
    /// queues for the PTY as one write.
    pending_replies: Vec<u8>,
    exit: ExitLatch,
    /// Whether the host last reported this terminal as focused.
    focused: bool,
}

impl<V: Vt> OrzmaTty<V> {
    /// Upper bound on chunks one [`Self::pump`] interprets: 64 reads of up
    /// to 4 KiB each, so at most about 256 KiB.
    pub const MAX_CHUNKS_PER_PUMP: usize = 64;

    /// Spawns `options.shell` under a new PTY and sizes the injected VT
    /// to the spawn geometry.
    ///
    /// The placements the initial sizing strands reach the next pump as a
    /// [`VtSignal::WebviewEvicted`] signal.
    pub fn spawn(vt: V, options: SpawnOptions) -> OrzmaTtyResult<Self> {
        let mut tty = Self::wired(vt, Pty::spawn(&options)?);
        tty.resize_vt(options.size);
        Ok(tty)
    }

    /// Reads the PTY master's current grid size back from the kernel
    /// (`TIOCGWINSZ`).
    ///
    /// # Panics
    ///
    /// Panics when the ioctl fails, which means the master fd is no
    /// longer valid.
    #[inline]
    pub fn pty_size(&self) -> PtySize {
        self.pty.size()
    }

    /// The working directory of the process this terminal is showing: its
    /// foreground process, else its shell. Only a directory that still
    /// exists is reported.
    ///
    /// Returns `None` when neither can be read: no process was spawned,
    /// the process belongs to another user, it has exited, its directory
    /// was removed, or the platform is not Unix.
    #[inline]
    pub fn process_cwd(&self) -> Option<PathBuf> {
        self.pty.process_cwd()
    }

    /// Read-only access to the VT, for host-side observation such as
    /// the display offset, modes, or cell contents.
    #[inline]
    pub fn vt(&self) -> &V {
        &self.vt
    }

    /// Builds a terminal around a fake PTY master instead of a spawned
    /// shell, so writes land on `writer` and no real PTY is opened.
    ///
    /// Resize calls still round-trip through [`Self::pty_size`], and no
    /// child process or reader thread is started, so everything the input
    /// methods emit can be observed on `writer` — typically a
    /// [`test_support::CaptureSink`] — once [`Self::settle_writes`]
    /// returns.
    ///
    /// The placements the initial sizing strands reach the next pump as a
    /// [`VtSignal::WebviewEvicted`] signal.
    ///
    /// Available only under `cfg(test)` in this crate and through the
    /// `test-support` feature downstream.
    #[cfg(any(test, feature = "test-support"))]
    pub fn detached(vt: V, size: GridSize, writer: Box<dyn Write + Send>) -> OrzmaTtyResult<Self> {
        let pty = Pty::with_master(Box::new(RecordingMaster::at(size).0), writer);
        let mut tty = Self::wired(vt, pty);
        tty.resize_vt(size);
        Ok(tty)
    }

    /// Like [`Self::detached`], but with the chunk and exit streams fed by
    /// the given receivers, so the caller controls the queued output and
    /// whether the child's exit is reported.
    #[cfg(any(test, feature = "test-support"))]
    pub fn detached_with_channels(
        vt: V,
        size: GridSize,
        writer: Box<dyn Write + Send>,
        chunk_rx: Receiver<Vec<u8>>,
        exit_rx: Receiver<Option<i32>>,
    ) -> OrzmaTtyResult<Self> {
        let pty = Pty::with_master_and_channels(
            Box::new(RecordingMaster::at(size).0),
            writer,
            chunk_rx,
            exit_rx,
        );
        let mut tty = Self::wired(vt, pty);
        tty.resize_vt(size);
        Ok(tty)
    }

    /// Feeds bytes through the same seam [`Self::pump`] runs PTY chunks
    /// through, arming the coalescer exactly as live output would.
    ///
    /// Available only under `cfg(test)` in this crate and through the
    /// `test-support` feature downstream.
    #[cfg(any(test, feature = "test-support"))]
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        self.feed_chunk(bytes);
    }

    /// Blocks until every input this terminal queued has been written to
    /// its PTY writer, or until the writer stops after a failure.
    ///
    /// Never returns while the writer is blocked in a write that does not
    /// complete.
    ///
    /// Available only under `cfg(test)` in this crate and through the
    /// `test-support` feature downstream.
    #[cfg(any(test, feature = "test-support"))]
    pub fn settle_writes(&self) {
        self.pty.settle_writes();
    }

    /// Scrolls the grid, arming the coalescer only when the viewport
    /// actually moved; a clamped or zero motion reports no damage.
    pub fn scroll(&mut self, scroll: Scroll) {
        if self.vt.scroll(scroll) {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }

    /// Anchors a selection, arming the coalescer only when the VT's
    /// state changed.
    pub fn start_selection(&mut self, cell: GridPoint, side: CellSide, kind: SelectionKind) {
        if self.vt.start_selection(cell, side, kind) {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }

    /// Moves the selection's moving end, arming the coalescer only when
    /// it moved.
    pub fn extend_selection(&mut self, cell: GridPoint, side: CellSide) {
        if self.vt.extend_selection(cell, side) {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }

    /// Drops the selection, arming the coalescer only when there was
    /// one.
    pub fn clear_selection(&mut self) {
        if self.vt.clear_selection() {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }

    /// Resizes both the PTY (kernel winsize, with `cell_px × cells` as
    /// the pixel extent) and the VT grid, then arms the coalescer so the
    /// new geometry repaints at the next deadline even on an otherwise
    /// idle terminal.
    ///
    /// When the PTY resize fails the call returns
    /// `OrzmaTtyError::PtyResize` and leaves the VT grid and coalescer
    /// untouched.
    ///
    /// A request for the grid size the VT already has changes nothing
    /// and reports no damage, so it arms nothing either.
    ///
    /// The placements the new geometry strands reach the next pump as a
    /// [`VtSignal::WebviewEvicted`] signal; no PTY output is needed to
    /// carry them.
    pub fn resize(&mut self, size: GridSize, cell_px: CellPixels) -> OrzmaTtyResult {
        self.pty.resize(size, cell_px)?;
        self.resize_vt(size);
        Ok(())
    }

    /// Removes the placements the host names, arming the coalescer only
    /// when one actually went.
    pub fn remove_placements(&mut self, instances: &[InstanceId]) {
        if self.vt.remove_placements(instances) {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }

    /// Registers a host-driven mount at the visible cell (`row`, `column`)
    /// and queues the VT's verdict as a signal: an accepted mount arms
    /// the coalescer and queues `WebviewMount`, a rejected one queues
    /// `WebviewMountRejected` without arming.
    pub fn mount_placement_at(
        &mut self,
        instance: InstanceId,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
    ) {
        let signal = if self.vt.mount_placement_at(row, column, size, instance) {
            self.coalescer.arm_or_extend(Instant::now());
            VtSignal::WebviewMount { instance, size }
        } else {
            VtSignal::WebviewMountRejected { instance }
        };
        self.pending_signals.push(TtySignal::Vt(signal));
    }

    /// Encodes a key press and queues it for the PTY.
    ///
    /// Snaps a scrolled-back viewport to the live tail first
    /// (scroll-on-input policy). `Ok` means the key was queued, not that it
    /// reached the PTY.
    ///
    /// # Errors
    ///
    /// Returns `PtyWriteQueueFull` when the PTY input queue has no room for
    /// the key (nothing is queued), `PtyWrite` once after the writer
    /// thread's write failed, and `PtyWriterClosed` after that.
    pub fn send_key(&mut self, key: &TerminalKey, mods: &TerminalModifiers) -> OrzmaTtyResult {
        let modes = self.vt.modes();
        self.snap_to_live_tail();
        self.pty
            .enqueue_write(PtyInput::encode_key(key, mods, modes).into_bytes())
    }

    /// Encodes one mouse report in the terminal's active mouse encoding
    /// and queues it for the PTY.
    ///
    /// Writes nothing while the VT has no mouse tracking level in force.
    ///
    /// Does not snap a scrolled-back viewport: the report's cell
    /// coordinates are the ones the host computed against the viewport on
    /// screen. `Ok` means the report was queued, not that it reached the
    /// PTY.
    ///
    /// # Errors
    ///
    /// Returns `PtyWriteQueueFull` when the PTY input queue has no room for
    /// the report (nothing is queued), `PtyWrite` once after the writer
    /// thread's write failed, and `PtyWriterClosed` after that.
    pub fn send_mouse(&mut self, report: MouseReport) -> OrzmaTtyResult {
        let modes = self.vt.modes();
        if !modes.mouse_reporting_active() {
            return Ok(());
        }
        self.pty
            .enqueue_write(PtyInput::encode_mouse(&report, modes.mouse_encoding).into_bytes())
    }

    /// Routes one frame's wheel notches by the VT's current modes and
    /// applies the result.
    ///
    /// Vertical notches become wheel reports while a mouse tracking level
    /// is in force and Shift is not held, cursor keys while alternate
    /// scroll is in effect, and a viewport scroll otherwise; horizontal
    /// notches become reports only. Cursor keys snap a scrolled-back
    /// viewport to the live tail first; reports and viewport scrolls leave
    /// it where it is. Whatever both axes encode is queued for the PTY as
    /// one write. `Ok` means the bytes were queued, not that they reached
    /// the PTY.
    ///
    /// # Errors
    ///
    /// Returns `PtyWriteQueueFull` when the PTY input queue has no room for
    /// the frame's bytes (nothing is queued), `PtyWrite` once after the
    /// writer thread's write failed, and `PtyWriterClosed` after that.
    pub fn send_wheel(&mut self, input: WheelInput, cfg: &WheelConfig) -> OrzmaTtyResult {
        let modes = self.vt.modes();
        let mut bytes = Vec::new();
        for decision in [
            WheelDecision::route(modes, input.up, input.mods, cfg),
            WheelDecision::route_horizontal(modes, input.right, input.mods, cfg),
        ] {
            self.stage_wheel_decision(&mut bytes, decision, input, modes);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.pty.enqueue_write(bytes)
    }

    /// Queues a paste of clipboard text for the PTY, honouring
    /// bracketed-paste mode (DECSET 2004).
    ///
    /// Empty text is a no-op: nothing is queued. Otherwise a scrolled-back
    /// viewport snaps to the live tail first (scroll-on-input policy), and
    /// the whole frame is queued as one write or rejected whole. `Ok` means
    /// the paste was queued, not that it reached the PTY.
    ///
    /// # Errors
    ///
    /// Returns `PtyWriteQueueFull` when the frame does not fit in the PTY
    /// input queue (nothing is queued), `PtyWrite` once after the writer
    /// thread's write failed, and `PtyWriterClosed` after that.
    pub fn send_paste(&mut self, text: &str) -> OrzmaTtyResult {
        if text.is_empty() {
            return Ok(());
        }
        let bracketed = self.vt.modes().bracketed_paste;
        self.snap_to_live_tail();
        self.pty
            .enqueue_write(PtyInput::encode_paste(text, bracketed).into_bytes())
    }

    /// Records whether the host gives this terminal focus, and reports a
    /// change to the application while it has focus reporting enabled
    /// (DECSET 1004): `CSI I` on gaining focus and `CSI O` on losing it.
    ///
    /// An unchanged state writes nothing, and enabling focus reporting
    /// reports nothing until the next change. The viewport does not move,
    /// and no repaint is scheduled.
    ///
    /// The new state is recorded even when the report is refused. `Ok`
    /// means the report was queued, not that it reached the PTY.
    ///
    /// # Errors
    ///
    /// Returns `PtyWriteQueueFull` when the PTY input queue has no room for
    /// the report (nothing is queued), `PtyWrite` once after the writer
    /// thread's write failed, and `PtyWriterClosed` after that.
    pub fn set_focused(&mut self, focused: bool) -> OrzmaTtyResult {
        if self.focused == focused {
            return Ok(());
        }
        self.focused = focused;
        if !self.vt.modes().focus_in_out {
            return Ok(());
        }
        self.pty
            .enqueue_write(PtyInput::encode_focus(focused).into_bytes())
    }

    /// The receivers to wait on for this terminal (see [`Readiness`]).
    pub fn readiness(&self) -> Readiness<'_> {
        let exit = match self.exit {
            ExitLatch::Running => Some(self.pty.exit_receiver()),
            ExitLatch::Observed(_) | ExitLatch::Reported => None,
        };
        Readiness {
            chunks: self.pty.chunk_receiver(),
            exit,
        }
    }

    /// How many output chunks wait unread in the PTY stream, in units
    /// of one reader `read(2)` result of up to 4 KiB.
    #[inline]
    pub fn pending_chunk_count(&self) -> usize {
        self.pty.chunk_receiver().len()
    }

    /// When the coalescer next wants a pump: `Some(now)` while the
    /// bootstrap frame is owed, the armed window's deadline while output
    /// is pending, `None` when idle.
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.coalescer.needs_bootstrap() {
            return Some(Instant::now());
        }
        self.coalescer.next_deadline()
    }

    /// Emits the pending signals and an immediate frame without reading
    /// the PTY or waiting for the coalesce window.
    ///
    /// Never reports `ChildExit`.
    pub fn flush_now(&mut self) -> PumpOutput {
        let signals = mem::take(&mut self.pending_signals);
        let frame = self.emit_frame();
        PumpOutput {
            frame,
            signals,
            more_pending: false,
        }
    }

    /// Drains the PTY and the VT into one output batch: interprets up to
    /// [`Self::MAX_CHUNKS_PER_PUMP`] queued chunks, queues pending replies
    /// for the PTY, surfaces buffered signals, and emits a frame when the
    /// coalesce window is due or the bootstrap snapshot is still owed.
    ///
    /// The child's exit is latched when observed and reported as a
    /// trailing `ChildExit` only on the pump that finds no chunk left, so
    /// the last output always precedes it. A reader thread that vanished
    /// without a status (both streams disconnected) reports
    /// `ChildExit { code: None }` once.
    pub fn pump(&mut self) -> PumpOutput {
        let chunks_disconnected = self.drain_chunks();
        let more_pending = !chunks_disconnected && self.pty.chunks_pending();
        self.latch_exit(chunks_disconnected);

        if !self.pending_replies.is_empty() {
            let replies = mem::take(&mut self.pending_replies);
            // NOTE: a refused reply is dropped, never waited on. A full queue
            // means the application stopped reading stdin, so waiting for
            // room would block `pump` until it reads again; a failed or
            // closed writer means the PTY is tearing down, which surfaces as
            // ChildExit.
            let _ = self.pty.enqueue_write(replies);
        }

        let mut signals = mem::take(&mut self.pending_signals);
        if !more_pending && let ExitLatch::Observed(code) = self.exit {
            signals.push(TtySignal::ChildExit { code });
            self.exit = ExitLatch::Reported;
        }

        let now = Instant::now();
        let frame = if self.coalescer.needs_bootstrap() || self.coalescer.is_due(now) {
            self.emit_frame()
        } else {
            None
        };
        PumpOutput {
            frame,
            signals,
            more_pending,
        }
    }

    /// Wires a VT to a PTY with an idle coalescer and nothing pending,
    /// leaving the VT at whatever size it arrived with.
    fn wired(vt: V, pty: Pty) -> Self {
        Self {
            vt,
            coalescer: Coalescer::default(),
            pty,
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
            exit: ExitLatch::Running,
            focused: false,
        }
    }

    /// Sizes the VT grid, arming the coalescer when the grid changed
    /// and queuing the placements the change stranded for the next
    /// pump.
    fn resize_vt(&mut self, size: GridSize) {
        let Some(changed) = self.vt.resize(size) else {
            return;
        };
        self.coalescer.arm_or_extend(Instant::now());
        if let Some(evicted) = VtSignal::evicted(changed.evicted) {
            self.pending_signals.push(TtySignal::Vt(evicted));
        }
    }

    /// Snaps a scrolled-back viewport to the live tail (scroll-on-input
    /// policy); a viewport already at the live tail stages no damage.
    fn snap_to_live_tail(&mut self) {
        if !self.vt.is_at_live_tail() {
            self.scroll(Scroll::Bottom);
        }
    }

    /// Appends the PTY bytes `decision` encodes to `bytes`, or applies its
    /// viewport scroll. A report needs `input.cell` and is dropped without
    /// one.
    fn stage_wheel_decision(
        &mut self,
        bytes: &mut Vec<u8>,
        decision: WheelDecision,
        input: WheelInput,
        modes: VtModes,
    ) {
        match decision {
            WheelDecision::Report { button, count } => {
                let Some(cell) = input.cell else {
                    return;
                };
                let report = MouseReport {
                    button,
                    kind: MouseReportKind::Press,
                    cell,
                    mods: input.report_mods,
                };
                let encoded = PtyInput::encode_mouse(&report, modes.mouse_encoding).into_bytes();
                for _ in 0..count {
                    bytes.extend_from_slice(&encoded);
                }
            }
            WheelDecision::CursorKeys { key, count } => {
                self.snap_to_live_tail();
                let encoded =
                    PtyInput::encode_key(&key, &TerminalModifiers::default(), modes).into_bytes();
                for _ in 0..count {
                    bytes.extend_from_slice(&encoded);
                }
            }
            WheelDecision::ScrollViewport(lines) => self.scroll(Scroll::Delta(lines)),
            WheelDecision::Noop => {}
        }
    }

    /// Interprets queued chunks up to the budget. Returns whether the
    /// output stream is disconnected (the reader thread is gone).
    fn drain_chunks(&mut self) -> bool {
        for _ in 0..Self::MAX_CHUNKS_PER_PUMP {
            match self.pty.poll_chunk() {
                ChunkPoll::Chunk(chunk) => self.feed_chunk(&chunk),
                ChunkPoll::Empty => return false,
                ChunkPoll::Disconnected => return true,
            }
        }
        false
    }

    /// Records the child's exit once it is observable, synthesizing a
    /// `None` status when both streams vanished without one.
    fn latch_exit(&mut self, chunks_disconnected: bool) {
        if self.exit != ExitLatch::Running {
            return;
        }
        match self.pty.poll_exit() {
            ExitPoll::Exited(code) => self.exit = ExitLatch::Observed(code),
            ExitPoll::Disconnected if chunks_disconnected => {
                self.exit = ExitLatch::Observed(None);
            }
            ExitPoll::Pending | ExitPoll::Disconnected => {}
        }
    }

    /// Asks the VT for a frame and settles the coalescer on success or
    /// disarms it when there was nothing to paint.
    fn emit_frame(&mut self) -> Option<Frame> {
        let frame = self.vt.frame();
        if frame.is_some() {
            self.coalescer.settle_emit();
        } else {
            self.coalescer.disarm();
        }
        frame
    }

    /// Interprets one PTY chunk, arms the coalescer when it staged
    /// damage, and buffers the update's signals and replies for the
    /// next pump.
    fn feed_chunk(&mut self, chunk: &[u8]) {
        let update = self.vt.interpret(chunk);
        if update.damaged {
            self.coalescer.arm_or_extend(Instant::now());
        }
        self.pending_signals
            .extend(update.signals.into_iter().map(TtySignal::Vt));
        self.pending_replies.extend(update.replies);
    }
}

#[cfg(test)]
mod tests;
