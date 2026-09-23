//! PTY-backed terminal core: spawns a shell under a PTY and
//! drives an injected [`Vt`] implementor behind a frame coalescer.

#![deny(unsafe_code)]

use crate::{
    coalescer::Coalescer,
    error::{OrzmaTtyError, OrzmaTtyResult},
    input::{
        MouseReport, MouseReportKind, PointerAction, PointerInput, PointerState, PtyInput,
        TerminalKey, TerminalModifiers, WheelConfig, WheelDecision, WheelInput,
    },
    pty::{ChunkPoll, ExitPoll, Pty},
    signal::TtySignal,
};
use crossbeam_channel::Receiver;
use orzma_vt::prelude::*;
use portable_pty::PtySize;
#[cfg(any(test, feature = "test-support"))]
use std::io::Write;
use std::mem;
use std::path::PathBuf;
use std::time::{Duration, Instant};
#[cfg(any(test, feature = "test-support"))]
use test_support::RecordingMaster;
use tracing::warn;

mod cell_pixels;
mod coalescer;
mod error;
mod input;
mod pty;
#[cfg(any(windows, test))]
mod shell_integration;
mod signal;
pub mod test_support;

pub use cell_pixels::CellPixels;

pub mod prelude {
    pub use crate::{
        CellPixels, OrzmaTty, PumpItem, PumpOutput, Readiness, error::*, input::*, signal::*,
    };
}

/// Spawn parameters consumed exactly once by `OrzmaTty::spawn`.
#[derive(Clone)]
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
    /// Whether orzma may make a shell it recognizes report its working
    /// directory. Has no effect outside Windows.
    pub shell_integration: bool,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvKey(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvValue(pub String);

/// One entry of a pump's output.
#[derive(Debug, Clone, PartialEq)]
pub enum PumpItem {
    /// A signal raised since the previous entry.
    Signal(TtySignal),
    /// A frame to draw. It reflects every signal listed ahead of it.
    Frame(Frame),
}

/// Everything one [`OrzmaTty::pump`] call produced.
pub struct PumpOutput {
    /// The signals and frames in the order they were produced; the
    /// consumer must forward them in this order. `ChildExit` is always
    /// last.
    pub items: Vec<PumpItem>,
    /// Whether output chunks remain queued after this pump's budget was
    /// spent, so the owner should pump again before waiting.
    pub more_pending: bool,
}

impl PumpOutput {
    /// The signals of [`Self::items`], in order.
    pub fn signals(&self) -> impl Iterator<Item = &TtySignal> {
        self.items.iter().filter_map(|item| match item {
            PumpItem::Signal(signal) => Some(signal),
            PumpItem::Frame(_) => None,
        })
    }

    /// The frames of [`Self::items`], in order.
    pub fn frames(&self) -> impl Iterator<Item = &Frame> {
        self.items.iter().filter_map(|item| match item {
            PumpItem::Frame(frame) => Some(frame),
            PumpItem::Signal(_) => None,
        })
    }
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
    /// Signals and frames produced since the previous pump, in order.
    pending: Vec<PumpItem>,
    /// Reply bytes produced by interpreted chunks, which the next pump
    /// queues for the PTY as one write.
    pending_replies: Vec<u8>,
    exit: ExitLatch,
    /// Whether the host last reported this terminal as focused.
    focused: bool,
    /// How each held pointer button routes, from its press to its release.
    pointer: PointerState,
    /// When the open synchronized update stops holding frames back;
    /// `None` while the VT reports none open. A deadline in the past
    /// marks an update that timed out and is not reopened until the VT
    /// reports it closed.
    sync_deadline: Option<Instant>,
}

impl<V: Vt> OrzmaTty<V> {
    /// Upper bound on chunks one [`Self::pump`] interprets: 64 reads of up
    /// to 4 KiB each, so at most about 256 KiB.
    pub const MAX_CHUNKS_PER_PUMP: usize = 64;

    /// How long an open synchronized update holds frames back.
    pub const SYNC_TIMEOUT: Duration = Duration::from_millis(150);

    /// The shortest time between a frame and the frame taken when a
    /// synchronized update closes.
    pub const SYNC_EMIT_INTERVAL: Duration = Duration::from_millis(12);

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

    /// The working directory of the process this terminal is showing: on
    /// Unix its foreground process, else its shell; on Windows its
    /// shell. Only a directory that still exists and can be entered is
    /// reported.
    ///
    /// Returns `None` when none can be read: no process was spawned, the
    /// process belongs to another user or is elevated, it has exited,
    /// its directory was removed or can no longer be entered, or the
    /// platform is none of macOS, Linux, and Windows.
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
    ///
    /// # Errors
    ///
    /// Reports a VT that interpreted none of a non-empty chunk, or more
    /// bytes than the chunk held.
    #[cfg(any(test, feature = "test-support"))]
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> OrzmaTtyResult {
        self.feed_chunk(bytes)
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
    ///
    /// When the viewport moves under a held selection drag, the
    /// selection's moving end follows it onto the cell now under the
    /// pointer.
    pub fn scroll(&mut self, scroll: Scroll) {
        if self.vt.scroll(scroll) {
            self.coalescer.arm_or_extend(Instant::now());
            self.follow_drag_end();
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
        self.pending.push(PumpItem::Signal(TtySignal::Vt(signal)));
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

    /// Routes one pointer event by the VT's current modes and applies the
    /// result: reports are queued for the PTY as one write, and selection
    /// effects apply to the VT.
    ///
    /// Every cell is clamped into the current grid first, including one
    /// kept from an earlier event such as the cell a cancel reports its
    /// releases at, and a viewport cell maps onto the grid at the current
    /// display offset. A press is forwarded only while a mouse tracking
    /// level is in force, Shift is not held, and the viewport is at the
    /// live tail, and that routing holds until the button's release.
    /// Nothing here moves the viewport. Returns the selected text when the
    /// event finished a selection drag; `None` otherwise, and when the
    /// selection is empty. `Ok` means the reports were queued, not that
    /// they reached the PTY.
    ///
    /// # Errors
    ///
    /// Returns `PtyWriteQueueFull` when the PTY input queue has no room for
    /// the reports (nothing is queued), `PtyWrite` once after the writer
    /// thread's write failed, and `PtyWriterClosed` after that. Selection
    /// effects apply either way.
    pub fn send_pointer(&mut self, mut input: PointerInput) -> OrzmaTtyResult<Option<String>> {
        let size = self.vt.grid_size();
        input.cell = input.cell.clamped_to(size);
        let modes = self.vt.modes();
        let offset = self.vt.display_offset();
        let at_live_tail = self.vt.is_at_live_tail();
        let mut bytes = Vec::new();
        let mut copied = None;
        for action in self.pointer.route(input, modes, at_live_tail) {
            match action {
                PointerAction::Report(mut report) => {
                    report.cell = report.cell.clamped_to(size);
                    bytes.extend(PtyInput::encode_mouse(&report, modes.mouse_encoding).into_bytes())
                }
                PointerAction::SelectionClear => self.clear_selection(),
                PointerAction::SelectionStart { cell, side, kind } => {
                    self.start_selection(cell.clamped_to(size).to_grid_point(offset), side, kind);
                }
                PointerAction::SelectionExtend { cell, side } => {
                    self.extend_selection(cell.clamped_to(size).to_grid_point(offset), side);
                }
                PointerAction::Copy => {
                    copied = self.vt.selection_text().filter(|text| !text.is_empty());
                }
            }
        }
        if !bytes.is_empty() {
            self.pty.enqueue_write(bytes)?;
        }
        Ok(copied)
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

    /// When this terminal next wants a pump, as of `now`: the open
    /// synchronized update's deadline while it holds frames back;
    /// otherwise `now` while the bootstrap frame is owed, the armed
    /// window's deadline while output is pending, and `None` when idle.
    ///
    /// The caller passes the same `now` it compares the result against,
    /// so a deadline this reports as due is due by that clock read.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        if self.holds_frames_back(now) {
            return self.sync_deadline;
        }
        if self.coalescer.needs_bootstrap() {
            return Some(now);
        }
        self.coalescer.next_deadline()
    }

    /// Returns the pending signals followed by an immediate frame, without
    /// reading the PTY or waiting for the coalesce window.
    ///
    /// Never reports `ChildExit`. An open synchronized update does not
    /// hold this frame back, and stays open.
    pub fn flush_now(&mut self) -> PumpOutput {
        self.emit_frame(Instant::now());
        PumpOutput {
            items: mem::take(&mut self.pending),
            more_pending: false,
        }
    }

    /// Drains the PTY and the VT into one output batch: interprets up to
    /// [`Self::MAX_CHUNKS_PER_PUMP`] queued chunks, queues pending replies
    /// for the PTY, surfaces the buffered signals, and lists a frame behind
    /// them when the coalesce window is due or the bootstrap snapshot is
    /// still owed.
    ///
    /// The child's exit is latched when observed and reported as the last
    /// item only on the pump that finds no chunk left, so the last output
    /// always precedes it. A reader thread that vanished without a status
    /// (both streams disconnected) reports `ChildExit { code: None }` once.
    ///
    /// No frame is emitted while a synchronized update is open and
    /// younger than [`Self::SYNC_TIMEOUT`]; a frame taken when an
    /// update closed is listed where it was taken.
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

        let now = Instant::now();
        if !self.holds_frames_back(now)
            && (self.coalescer.needs_bootstrap() || self.coalescer.is_due(now))
        {
            self.emit_frame(now);
        }
        let mut items = mem::take(&mut self.pending);
        if !more_pending && let ExitLatch::Observed(code) = self.exit {
            items.push(PumpItem::Signal(TtySignal::ChildExit { code }));
            self.exit = ExitLatch::Reported;
        }
        PumpOutput {
            items,
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
            pending: Vec::new(),
            pending_replies: Vec::new(),
            exit: ExitLatch::Running,
            focused: false,
            pointer: PointerState::default(),
            sync_deadline: None,
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
            self.pending.push(PumpItem::Signal(TtySignal::Vt(evicted)));
        }
    }

    /// Snaps a scrolled-back viewport to the live tail (scroll-on-input
    /// policy); a viewport already at the live tail stages no damage.
    fn snap_to_live_tail(&mut self) {
        if !self.vt.is_at_live_tail() {
            self.scroll(Scroll::Bottom);
        }
    }

    /// Moves a held selection drag's moving end onto the grid point its
    /// viewport cell shows at the current display offset.
    fn follow_drag_end(&mut self) {
        let Some((cell, side)) = self.pointer.drag_end() else {
            return;
        };
        let point = cell
            .clamped_to(self.vt.grid_size())
            .to_grid_point(self.vt.display_offset());
        self.extend_selection(point, side);
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
                ChunkPoll::Chunk(chunk) => {
                    if let Err(error) = self.feed_chunk(&chunk) {
                        warn!(%error, "dropping the rest of the chunk");
                    }
                }
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

    /// Asks the VT for a frame and queues it behind the pending signals,
    /// settling the coalescer on success or disarming it when there was
    /// nothing to paint.
    fn emit_frame(&mut self, now: Instant) {
        match self.vt.frame() {
            Some(frame) => {
                self.coalescer.settle_emit(now);
                self.pending.push(PumpItem::Frame(frame));
            }
            None => self.coalescer.disarm(),
        }
    }

    /// Interprets one PTY chunk to its end, resubmitting the rest after
    /// each closed synchronized update, and buffers what it produced for
    /// the next pump. A close takes a frame at once unless one was
    /// emitted within [`Self::SYNC_EMIT_INTERVAL`].
    ///
    /// # Errors
    ///
    /// Reports a VT that interpreted none of a non-empty chunk, or more
    /// bytes than the chunk held. What the call already buffered stays
    /// buffered, and the rest of the chunk is left uninterpreted.
    fn feed_chunk(&mut self, chunk: &[u8]) -> OrzmaTtyResult {
        let mut rest = chunk;
        while !rest.is_empty() {
            let update = self.vt.interpret(rest);
            let now = Instant::now();
            if update.damaged {
                self.coalescer.arm_or_extend(now);
            }
            self.pending.extend(
                update
                    .signals
                    .into_iter()
                    .map(|signal| PumpItem::Signal(TtySignal::Vt(signal))),
            );
            self.pending_replies.extend(update.replies);
            self.track_synchronized_update(now);
            if update.synchronized_update_closed
                && !self.coalescer.emitted_within(Self::SYNC_EMIT_INTERVAL, now)
            {
                self.emit_frame(now);
            }
            if update.consumed == 0 {
                return Err(OrzmaTtyError::VtConsumedNothing { len: rest.len() });
            }
            if update.consumed > rest.len() {
                return Err(OrzmaTtyError::VtConsumedBeyondChunk {
                    consumed: update.consumed,
                    len: rest.len(),
                });
            }
            rest = &rest[update.consumed..];
        }
        Ok(())
    }

    /// Opens the deadline when the VT reports a synchronized update and
    /// none is tracked, and clears it when the VT reports none.
    fn track_synchronized_update(&mut self, now: Instant) {
        if !self.vt.modes().synchronized_output.is_active() {
            self.sync_deadline = None;
        } else if self.sync_deadline.is_none() {
            self.sync_deadline = Some(now + Self::SYNC_TIMEOUT);
        }
    }

    /// Whether an open synchronized update still holds frames back at
    /// `now`.
    fn holds_frames_back(&self, now: Instant) -> bool {
        self.sync_deadline.is_some_and(|deadline| now < deadline)
    }
}

#[cfg(test)]
mod tests;
