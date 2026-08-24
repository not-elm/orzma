//! PTY-backed terminal core: spawns the login shell under a PTY and
//! drives an injected [`Vt`] implementor behind a frame coalescer.

use crate::{
    coalescer::Coalescer,
    error::OrzmaTtyResult,
    input::{MouseReport, PtyInput, TerminalKey, TerminalModifiers},
    pty::Pty,
    signal::TtySignal,
};
use orzma_vt::prelude::*;
use portable_pty::PtySize;
use std::path::PathBuf;
use std::{io::Write, mem, time::Instant};

mod coalescer;
mod error;
mod input;
mod pty;
mod signal;
pub mod test_support;

pub mod prelude {
    pub use crate::{OrzmaTty, error::*, input::*, signal::*};
}

/// Spawn parameters consumed exactly once by `OrzmaTty::spawn`.
pub struct SpawnOptions {
    /// Terminal column count.
    pub cols: u16,
    /// Terminal row count.
    pub rows: u16,
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

pub struct InterpretOutput {
    pub frame: Option<Frame>,
    pub signals: Vec<TtySignal>,
}

/// A live terminal: the VT emulation plus the PTY it is wired to.
pub struct OrzmaTty<V: Vt> {
    vt: V,
    coalescer: Coalescer,
    pty: Pty,
    /// Signals produced by interpreted chunks, awaiting the next pump.
    pending_signals: Vec<TtySignal>,
    /// Reply bytes produced by interpreted chunks, awaiting one PTY
    /// write in the next pump.
    pending_replies: Vec<u8>,
}

impl<V: Vt> OrzmaTty<V> {
    /// Upper bound for a resize's column count; requests beyond it are
    /// ignored by [`Self::resize`].
    ///
    /// 4096 columns is beyond any real display (8K at a tiny font is
    /// ~2000), while capping the VT grid allocation a degenerate or
    /// hostile request could otherwise trigger.
    const MAX_COLS: u16 = 4096;
    /// Upper bound for a resize's row count; requests beyond it are
    /// ignored by [`Self::resize`]. Same rationale as [`Self::MAX_COLS`].
    const MAX_ROWS: u16 = 4096;

    /// Spawns the login shell under a new PTY and sizes the injected VT
    /// to the spawn geometry.
    pub fn spawn(mut vt: V, options: SpawnOptions) -> OrzmaTtyResult<Self> {
        let pty = Pty::spawn(&options)?;
        vt.resize(GridSize {
            cols: options.cols,
            rows: options.rows,
        });
        Ok(Self {
            vt,
            coalescer: Coalescer::default(),
            pty,
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
        })
    }

    /// Reads the PTY master's current grid size back from the kernel
    /// (`TIOCGWINSZ`).
    ///
    /// # Panics
    ///
    /// Panics when the ioctl fails, which means the master fd is no
    /// longer valid and the terminal is unusable anyway.
    #[inline]
    pub fn pty_size(&self) -> PtySize {
        self.pty.size()
    }

    /// Read-only access to the VT, for host-side observation such as
    /// the display offset, modes, or cell contents.
    #[inline]
    pub fn vt(&self) -> &V {
        &self.vt
    }

    /// Builds a terminal whose PTY writes land on `writer` instead of a
    /// spawned shell.
    ///
    /// A PTY is still opened at the grid size, but no child process or
    /// reader thread is started, so everything the input methods emit
    /// can be observed on `writer` — typically a
    /// [`test_support::CaptureSink`].
    pub fn detached(
        mut vt: V,
        cols: u16,
        rows: u16,
        writer: Box<dyn Write + Send>,
    ) -> OrzmaTtyResult<Self> {
        let pty = Pty::detached(cols, rows, writer)?;
        vt.resize(GridSize { cols, rows });
        Ok(Self {
            vt,
            coalescer: Coalescer::default(),
            pty,
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
        })
    }

    /// Feeds bytes through the same seam [`Self::pump`] runs PTY chunks
    /// through, arming the coalescer exactly as live output would.
    ///
    /// Available to tests only: in-crate under `cfg(test)`, downstream
    /// via the `test-support` feature.
    #[cfg(any(test, feature = "test-support"))]
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        self.feed_chunk(bytes);
    }

    /// Scrolls the grid, arming the coalescer only when the viewport
    /// actually moved — a clamped or zero motion reports no damage, and
    /// arming for it would open an emit window for a repaint that never
    /// comes.
    pub fn scroll(&mut self, scroll: Scroll) {
        if self.vt.scroll(scroll) {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }

    /// Resizes both the PTY (kernel winsize) and the VT grid, then arms
    /// the coalescer so the reflow repaints at the next deadline even
    /// on an otherwise idle terminal.
    ///
    /// A request with a zero axis, or one exceeding [`Self::MAX_COLS`] /
    /// [`Self::MAX_ROWS`], is ignored with `Ok` — neither clamped nor
    /// an error. When the PTY resize fails the call returns
    /// [`OrzmaTtyError::PtyResize`] and leaves the VT grid and
    /// coalescer untouched (PTY first; nothing changes on failure).
    ///
    /// A request for the grid size the VT already has reflows nothing
    /// and reports no damage, so it arms nothing either — the same gate
    /// [`Self::scroll`] applies to a clamped motion.
    pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTtyResult {
        if cols == 0 || rows == 0 || Self::MAX_COLS < cols || Self::MAX_ROWS < rows {
            return Ok(());
        }
        self.pty.resize(cols, rows)?;
        if self.vt.resize(GridSize { cols, rows }) {
            self.coalescer.arm_or_extend(Instant::now());
        }
        Ok(())
    }

    /// Encodes a key press and writes it to the PTY.
    ///
    /// Snaps a scrolled-back viewport to the live tail first
    /// (scroll-on-input policy) so the echo is visible.
    pub fn send_key(&mut self, key: &TerminalKey, mods: &TerminalModifiers) -> OrzmaTtyResult {
        let modes = self.vt.modes();
        self.snap_to_live_tail();
        self.pty
            .write_all(PtyInput::encode_key(key, mods, modes.app_cursor).as_bytes())
    }

    /// Encodes one mouse report in the terminal's active mouse encoding
    /// and writes it to the PTY.
    ///
    /// Deliberately does NOT snap a scrolled-back viewport: the
    /// report's cell coordinates were computed by the host against the
    /// viewport the user is looking at, so yanking the view to the live
    /// tail on every report would make the screen jump under the
    /// pointer.
    pub fn send_mouse(&mut self, report: MouseReport) -> OrzmaTtyResult {
        let sequence = report.encode(self.vt.modes().mouse_encoding);
        self.pty.write_all(&sequence)
    }

    /// Writes a paste of clipboard text to the PTY, honouring
    /// bracketed-paste mode (DECSET 2004) via [`PtyInput::encode_paste`].
    ///
    /// Empty text is a no-op: nothing reaches the PTY. Otherwise a
    /// scrolled-back viewport snaps to the live tail first
    /// (scroll-on-input policy), and the whole frame goes out in a
    /// single write — a partially-written frame would leave the
    /// receiving app inside an unterminated paste.
    pub fn send_paste(&mut self, text: &str) -> OrzmaTtyResult {
        if text.is_empty() {
            return Ok(());
        }
        let bracketed = self.vt.modes().bracketed_paste;
        self.snap_to_live_tail();
        self.pty
            .write_all(PtyInput::encode_paste(text, bracketed).as_bytes())
    }

    /// Drains the PTY and the VT into one output batch: interprets
    /// queued chunks, writes pending replies back to the PTY, surfaces
    /// buffered signals (with `ChildExit` last), and emits a frame when
    /// the coalesce window is due.
    pub fn pump(&mut self) -> InterpretOutput {
        self.drain_chunks();
        let exit = self.pty.try_recv_exit();
        if exit.is_some() {
            // NOTE: the reader thread sends every chunk before the exit
            // report, so one more drain here closes the race where the
            // final output lands between the drain above and the poll —
            // without it, a host that tears down on ChildExit loses the
            // child's last output.
            self.drain_chunks();
        }

        if !self.pending_replies.is_empty() {
            let replies = mem::take(&mut self.pending_replies);
            // NOTE: a failed reply write is dropped deliberately — a PTY
            // that rejects writes is tearing down and surfaces as
            // ChildExit; there is no receiver left to answer.
            let _ = self.pty.write_all(&replies);
        }

        let mut signals = mem::take(&mut self.pending_signals);
        if let Some(code) = exit {
            signals.push(TtySignal::ChildExit { code });
        }

        let mut frame: Option<Frame> = None;
        if self.coalescer.is_due(Instant::now()) {
            if let Some(f) = self.vt.frame() {
                frame.replace(f);
            }
            self.coalescer.disarm();
        }
        InterpretOutput { frame, signals }
    }

    /// Snaps a scrolled-back viewport to the live tail (scroll-on-input
    /// policy), gated on [`Vt::is_at_live_tail`] so a no-op call stages
    /// no damage.
    fn snap_to_live_tail(&mut self) {
        if !self.vt.is_at_live_tail() {
            self.scroll(Scroll::Bottom);
        }
    }

    /// Drains every queued PTY chunk into the VT.
    fn drain_chunks(&mut self) {
        while let Some(chunk) = self.pty.try_read_chunk() {
            self.feed_chunk(&chunk);
        }
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
mod tests {
    use super::*;
    use crate::error::OrzmaTtyError;
    use crate::test_support::{CaptureSink, FailingMaster, FakeVt, RecordingMaster};
    use crossbeam_channel::{Sender, unbounded};

    /// Mirrors [`OrzmaTty::detached`] over a fake master instead of a
    /// real one.
    ///
    /// Opening a real one made every test sharing the run flaky.
    /// Cycling master/slave pairs as fast as the parallel harness does
    /// outruns the kernel's reclamation of pty slots, and `openpty`
    /// then fails with `ENXIO`; measured on macOS at 5-10 failures per
    /// 960 concurrent calls, and at zero once the slave side is left
    /// out.
    fn detached_term() -> (OrzmaTty<FakeVt>, CaptureSink) {
        let sink = CaptureSink::default();
        let (master, _) = RecordingMaster::at(80, 24);
        let mut vt = FakeVt::new(80, 24);
        vt.resize(GridSize { cols: 80, rows: 24 });
        let term = OrzmaTty {
            vt,
            coalescer: Coalescer::default(),
            pty: Pty::with_master(Box::new(master), Box::new(sink.clone())),
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
        };
        (term, sink)
    }

    fn failing_term() -> OrzmaTty<FakeVt> {
        OrzmaTty {
            vt: FakeVt::new(80, 24),
            coalescer: Coalescer::default(),
            pty: Pty::with_master(Box::new(FailingMaster), Box::new(CaptureSink::default())),
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
        }
    }

    /// A terminal whose PTY chunk and exit streams are fed by the
    /// returned senders, so tests can inject output and child-exit
    /// reports.
    fn channelled_term() -> (OrzmaTty<FakeVt>, Sender<Vec<u8>>, Sender<Option<i32>>) {
        let (chunk_tx, chunk_rx) = unbounded();
        let (exit_tx, exit_rx) = unbounded();
        let term = OrzmaTty {
            vt: FakeVt::new(80, 24),
            coalescer: Coalescer::default(),
            pty: Pty::with_master_and_channels(
                Box::new(FailingMaster),
                Box::new(CaptureSink::default()),
                chunk_rx,
                exit_rx,
            ),
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
        };
        (term, chunk_tx, exit_tx)
    }

    /// Collects the `ChildExit` codes out of a pumped signal batch.
    fn child_exits(signals: &[TtySignal]) -> Vec<Option<i32>> {
        signals
            .iter()
            .filter_map(|signal| match signal {
                TtySignal::ChildExit { code } => Some(*code),
                _ => None,
            })
            .collect()
    }

    fn sizes(term: &OrzmaTty<FakeVt>) -> ((u16, u16), (u16, u16)) {
        let pty = term.pty_size();
        let grid = term.vt.grid_size();
        ((pty.cols, pty.rows), (grid.cols, grid.rows))
    }

    /// Asserts that a resize reaches both seams: the PTY size read back
    /// from the kernel and the `GridSize` handed to the VT.
    ///
    /// Case: the user drags the window to a new size.
    #[test]
    fn resize_applies_the_size_to_both_seams() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert_eq!(sizes(&term), ((120, 40), (120, 40)));
        assert_eq!(
            term.vt.resizes.last(),
            Some(&GridSize {
                cols: 120,
                rows: 40
            })
        );
    }

    /// Asserts that a resize never writes through the PTY writer.
    ///
    /// The size change reaches the child as a kernel ioctl, so the
    /// decided policy is that nothing at all enters the byte stream —
    /// not even an XTWINOPS report.
    ///
    /// Case: the user resizes the window while a program is reading
    /// stdin.
    #[test]
    fn resize_does_not_write_through_the_pty_writer() {
        let (mut term, sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts that a successful resize arms the coalescer.
    ///
    /// Case: the user resizes the window at an idle shell prompt, where
    /// the reflow is the only thing that changes.
    #[test]
    fn resize_arms_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert!(term.coalescer.is_armed());
    }

    /// Asserts that a zero-axis request touches neither the PTY size
    /// nor the VT.
    ///
    /// The agreed policy is to ignore such a request outright rather
    /// than clamp it.
    ///
    /// Case: a minimized window, or a frame before cell metrics load,
    /// computes 0 for an axis.
    #[test]
    fn a_zero_axis_resize_is_ignored() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        let baseline = term.vt.resizes.len();
        for (cols, rows) in [(0, 0), (0, 40), (120, 0)] {
            term.resize(cols, rows).expect("ignored resize must be Ok");
            assert_eq!(
                sizes(&term),
                ((120, 40), (120, 40)),
                "resize {cols}x{rows} must be ignored"
            );
        }
        assert_eq!(term.vt.resizes.len(), baseline);
    }

    /// Asserts the per-axis cap: requests beyond `MAX_COLS` /
    /// `MAX_ROWS` are ignored, the boundary value is applied.
    ///
    /// Case: a degenerate or hostile window geometry asks for a grid
    /// far larger than any real display.
    #[test]
    fn an_oversized_axis_resize_is_ignored() {
        const MAX_COLS: u16 = OrzmaTty::<FakeVt>::MAX_COLS;
        const MAX_ROWS: u16 = OrzmaTty::<FakeVt>::MAX_ROWS;
        let (mut term, _sink) = detached_term();
        for (cols, rows) in [(MAX_COLS + 1, 24), (80, MAX_ROWS + 1)] {
            term.resize(cols, rows).expect("ignored resize must be Ok");
            assert_eq!(
                sizes(&term),
                ((80, 24), (80, 24)),
                "resize {cols}x{rows} must be ignored"
            );
        }
        term.resize(MAX_COLS, 24).expect("resize");
        assert_eq!(sizes(&term), ((MAX_COLS, 24), (MAX_COLS, 24)));
    }

    /// Asserts that an ignored request does not arm the coalescer.
    ///
    /// Case: a minimized window emits a stream of zero-axis requests.
    #[test]
    fn an_ignored_resize_does_not_arm_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(0, 40).expect("ignored resize must be Ok");
        term.resize(OrzmaTty::<FakeVt>::MAX_COLS + 1, 24)
            .expect("ignored resize must be Ok");
        assert!(!term.coalescer.is_armed());
    }

    /// Asserts that a resize to the size the terminal already has arms
    /// nothing.
    ///
    /// Case: the host recomputes cells after a pixel-only window change
    /// and re-applies the grid size the VT already holds.
    #[test]
    fn a_same_size_resize_does_not_arm_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(80, 24).expect("same-size resize must be Ok");
        assert!(!term.coalescer.is_armed());
    }

    /// Asserts that a same-size resize leaves an already-open emit
    /// window's deadline untouched.
    ///
    /// Case: a burst of window events re-applies the current grid size
    /// while an earlier repaint is still pending.
    #[test]
    fn a_same_size_resize_does_not_extend_the_deadline() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        let deadline = term.coalescer.next_deadline();
        assert!(deadline.is_some(), "precondition: a real resize arms");
        term.resize(120, 40).expect("same-size resize must be Ok");
        assert_eq!(term.coalescer.next_deadline(), deadline);
    }

    /// Asserts that back-to-back resizes settle on the last requested
    /// size on both seams.
    ///
    /// Case: a live window drag fires a burst of requests.
    #[test]
    fn sequential_resizes_settle_on_the_last_size() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        term.resize(90, 30).expect("resize");
        assert_eq!(sizes(&term), ((90, 30), (90, 30)));
    }

    /// Asserts PTY-first ordering via failure atomicity: when the PTY
    /// ioctl fails, the call returns `PtyResize` and the VT and
    /// coalescer are untouched.
    ///
    /// Case: the kernel refuses the winsize ioctl, and the renderer
    /// must keep drawing the size the child still has.
    #[test]
    fn a_failing_pty_resize_leaves_the_vt_untouched() {
        let mut term = failing_term();
        let result = term.resize(120, 40);
        assert!(
            matches!(result, Err(OrzmaTtyError::PtyResize(_))),
            "expected PtyResize, got {result:?}"
        );
        assert!(term.vt.resizes.is_empty());
        assert!(!term.coalescer.is_armed());
    }

    /// Asserts that a scroll the VT reports as a real move arms the
    /// coalescer.
    ///
    /// Case: the user scrolls into history on an idle terminal, where
    /// the viewport change is the only thing that happens.
    #[test]
    fn scroll_arms_the_coalescer_when_the_viewport_moves() {
        let (mut term, _sink) = detached_term();
        term.vt.scroll_moves = true;
        term.scroll(Scroll::Delta(3));
        assert!(term.coalescer.is_armed());
    }

    /// Asserts that a scroll the VT reports as a no-op arms nothing.
    ///
    /// Case: the user keeps turning the wheel after the viewport
    /// reached the end of the scrollback.
    #[test]
    fn a_no_op_scroll_does_not_arm_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.scroll(Scroll::Delta(5));
        assert!(!term.coalescer.is_armed());
    }

    /// Asserts that a no-op scroll leaves an already-open emit window's
    /// deadline untouched.
    ///
    /// Case: the user keeps spinning the wheel at the clamp while an
    /// earlier repaint is still pending.
    #[test]
    fn a_no_op_scroll_does_not_extend_the_deadline() {
        let (mut term, _sink) = detached_term();
        term.vt.scroll_moves = true;
        term.scroll(Scroll::Delta(3));
        let deadline = term.coalescer.next_deadline();
        assert!(deadline.is_some(), "precondition: a real scroll arms");
        term.vt.scroll_moves = false;
        term.scroll(Scroll::Delta(0));
        assert_eq!(term.coalescer.next_deadline(), deadline);
    }

    /// Asserts that scrolling writes nothing through the PTY writer.
    ///
    /// Viewport motion is host-side state, so the decided policy is
    /// that no bytes reach the child — neither a CSI S/T pair nor
    /// arrow keys.
    ///
    /// Case: the user scrolls through history while a program is
    /// reading stdin.
    #[test]
    fn scroll_writes_nothing_through_the_pty_writer() {
        let (mut term, sink) = detached_term();
        term.vt.scroll_moves = true;
        term.scroll(Scroll::Delta(3));
        term.scroll(Scroll::Bottom);
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts the scroll-on-input integration: user input while
    /// scrolled back snaps the viewport to the live tail AND schedules
    /// the repaint of that snap.
    ///
    /// Case: the user scrolls into history and then pastes, expecting
    /// the view to jump back to the prompt where the echo lands.
    #[test]
    fn paste_while_scrolled_back_snaps_and_arms() {
        let (mut term, _sink) = detached_term();
        term.vt.display_offset = DisplayOffset(3);
        term.send_paste("x").expect("send_paste");
        assert!(
            term.vt.scrolls.iter().any(|s| matches!(s, Scroll::Bottom)),
            "input must snap to the live tail"
        );
        assert_eq!(term.vt.display_offset, DisplayOffset(0));
        assert!(
            term.coalescer.is_armed(),
            "the snap must schedule a repaint"
        );
    }

    /// Asserts that key encoding consults the VT-reported DECCKM state.
    ///
    /// Case: an arrow key pressed in a full-screen app that enabled
    /// application cursor keys, then again at a plain prompt.
    #[test]
    fn send_key_honours_the_vt_reported_cursor_mode() {
        let (mut term, sink) = detached_term();
        term.vt.modes.app_cursor = true;
        term.send_key(&TerminalKey::ArrowUp, &TerminalModifiers::default())
            .expect("send_key");
        assert_eq!(sink.contents(), b"\x1bOA");

        let (mut term, sink) = detached_term();
        term.send_key(&TerminalKey::ArrowUp, &TerminalModifiers::default())
            .expect("send_key");
        assert_eq!(sink.contents(), b"\x1b[A");
    }

    /// Asserts that paste encoding consults the VT-reported bracketed
    /// paste mode.
    ///
    /// Case: pasting into an app that enabled DECSET 2004 (vim, fzf,
    /// modern shells).
    #[test]
    fn send_paste_honours_the_vt_reported_bracketed_mode() {
        let (mut term, sink) = detached_term();
        term.vt.modes.bracketed_paste = true;
        term.send_paste("hi").expect("send_paste");
        assert_eq!(sink.contents(), b"\x1b[200~hi\x1b[201~");
    }

    /// Asserts that a detached terminal's PTY writes land on the
    /// injected sink, byte-identical.
    ///
    /// Case: a caller builds a terminal with an injected writer instead
    /// of a spawned shell, then reads back the bytes the terminal
    /// produced.
    #[test]
    fn detached_routes_writes_to_the_injected_sink() {
        let (mut term, sink) = detached_term();
        term.send_paste("hi").expect("send_paste");
        assert_eq!(sink.contents(), b"hi");
    }

    /// Asserts that `send_paste("")` writes nothing at all.
    ///
    /// The decided policy is to write nothing rather than an empty
    /// bracketed-paste frame, which would still wake the receiving
    /// program.
    ///
    /// Case: the user pastes with an empty clipboard.
    #[test]
    fn empty_paste_writes_nothing_to_the_pty() {
        let (mut term, sink) = detached_term();
        term.send_paste("").expect("send_paste");
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts that a pending child-exit report surfaces in `pump`'s
    /// signals as `ChildExit` carrying the reported code.
    ///
    /// Case: the child exits with a nonzero code while the terminal is
    /// otherwise idle, and the host pumps on the next frame.
    #[test]
    fn pump_surfaces_child_exit_with_the_reported_code() {
        let (mut term, _chunk_tx, exit_tx) = channelled_term();
        exit_tx.send(Some(3)).expect("send exit");
        assert_eq!(child_exits(&term.pump().signals), vec![Some(3)]);
    }

    /// Asserts that a failed `wait` surfaces as `ChildExit` with
    /// `code: None` rather than being dropped.
    ///
    /// Case: the reader thread's `wait` on the exited child fails, so
    /// no exit code exists to report.
    #[test]
    fn pump_surfaces_a_wait_failure_as_code_none() {
        let (mut term, _chunk_tx, exit_tx) = channelled_term();
        exit_tx.send(None).expect("send exit");
        assert_eq!(child_exits(&term.pump().signals), vec![None]);
    }

    /// Asserts that `ChildExit` appears in exactly one `pump` result
    /// and never again on later calls.
    ///
    /// Case: the shell exits while the host keeps pumping every frame.
    #[test]
    fn child_exit_is_emitted_exactly_once_across_pumps() {
        let (mut term, _chunk_tx, exit_tx) = channelled_term();
        exit_tx.send(Some(0)).expect("send exit");
        assert_eq!(child_exits(&term.pump().signals), vec![Some(0)]);
        for _ in 0..3 {
            assert_eq!(child_exits(&term.pump().signals), vec![]);
        }
    }

    /// Asserts that a detached terminal never emits `ChildExit`.
    ///
    /// Its exit channel's sender is dropped at construction, so `pump`
    /// observes a disconnected channel; the decided reading of that
    /// state is "there is no child to report on", never "the child
    /// died".
    ///
    /// Case: a detached test terminal is pumped every frame like a
    /// live one.
    #[test]
    fn pump_on_a_detached_terminal_never_emits_child_exit() {
        let (mut term, _sink) = detached_term();
        for _ in 0..3 {
            assert_eq!(child_exits(&term.pump().signals), vec![]);
        }
    }

    /// Asserts that a `pump` which reports `ChildExit` has already
    /// interpreted every pending output chunk.
    ///
    /// Case: `echo bye` — the reader thread delivers the final output
    /// chunk and then the exit report, and the host pumps once after
    /// both arrived.
    #[test]
    fn the_final_output_is_interpreted_when_the_exit_is_reported() {
        let (mut term, chunk_tx, exit_tx) = channelled_term();
        chunk_tx.send(b"bye".to_vec()).expect("send chunk");
        exit_tx.send(Some(0)).expect("send exit");
        assert_eq!(child_exits(&term.pump().signals), vec![Some(0)]);
        assert!(term.vt.interpreted.contains(&b"bye".to_vec()));
    }

    /// Asserts that VT signals from interpreted chunks surface as
    /// `TtySignal::Vt`, ahead of a `ChildExit` in the same batch.
    ///
    /// Case: the shell rings the bell in its final output and exits;
    /// the host must observe the bell before acting on the exit.
    #[test]
    fn vt_signals_are_forwarded_before_child_exit() {
        let (mut term, chunk_tx, exit_tx) = channelled_term();
        term.vt.updates.push_back(VtUpdate {
            damaged: true,
            signals: vec![VtSignal::Bell],
            replies: Vec::new(),
        });
        chunk_tx.send(b"\x07".to_vec()).expect("send chunk");
        exit_tx.send(Some(0)).expect("send exit");
        let signals = term.pump().signals;
        assert_eq!(
            signals,
            vec![
                TtySignal::Vt(VtSignal::Bell),
                TtySignal::ChildExit { code: Some(0) }
            ]
        );
    }

    /// Asserts that reply bytes from interpreted chunks are written
    /// back to the PTY by the next pump.
    ///
    /// Case: an application sends a DSR cursor-position query and
    /// blocks until the report arrives.
    #[test]
    fn replies_are_written_back_to_the_pty() {
        let (chunk_tx, chunk_rx) = unbounded();
        let (_exit_tx, exit_rx) = unbounded();
        let sink = CaptureSink::default();
        let mut term = OrzmaTty {
            vt: FakeVt::new(80, 24),
            coalescer: Coalescer::default(),
            pty: Pty::with_master_and_channels(
                Box::new(FailingMaster),
                Box::new(sink.clone()),
                chunk_rx,
                exit_rx,
            ),
            pending_signals: Vec::new(),
            pending_replies: Vec::new(),
        };
        term.vt.updates.push_back(VtUpdate {
            damaged: true,
            signals: Vec::new(),
            replies: b"\x1b[1;1R".to_vec(),
        });
        chunk_tx.send(b"\x1b[6n".to_vec()).expect("send chunk");
        term.pump();
        assert_eq!(sink.contents(), b"\x1b[1;1R");
    }

    /// Asserts that a chunk which stages no damage leaves the coalesce
    /// window closed.
    ///
    /// Arming on every non-empty chunk would wake the emit path for
    /// output that changes nothing on screen, which is what the old
    /// `verdict.is_some()` check did.
    ///
    /// Case: a program queries the cursor position, so the VT answers
    /// with reply bytes and touches no cell.
    #[test]
    fn a_chunk_that_stages_no_damage_does_not_arm_the_window() {
        let (mut term, _sink) = detached_term();
        term.vt.updates.push_back(VtUpdate {
            damaged: false,
            signals: Vec::new(),
            replies: b"\x1b[1;1R".to_vec(),
        });
        term.feed_bytes(b"\x1b[6n");
        assert!(!term.coalescer.is_armed());
    }
}
