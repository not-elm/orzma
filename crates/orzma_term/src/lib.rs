//! PTY-backed terminal core: spawns the login shell under a PTY and
//! drives an [`OrzmaVt`] behind a frame coalescer.

use crate::{
    coalescer::Coalescer,
    error::{OrzmaTermError, OrzmaTermResult},
    input::{MouseButton, MouseReport, PtyInput, TerminalKey, TerminalModifiers},
    pty::Pty,
    signal::TermSignal,
};
use orzma_vt::prelude::*;
use portable_pty::PtySize;
use std::path::PathBuf;
use std::{io::Write, time::Instant};

mod coalescer;
mod error;
mod input;
mod pty;
mod signal;
pub mod test_support;

pub mod prelude {
    pub use crate::{OrzmaTerm, error::*, input::*, signal::*};
}

/// Spawn parameters consumed exactly once by `OrzmaTerm::spawn`.
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
    pub signals: Vec<TermSignal>,
}

/// A live terminal: the VT emulation plus the PTY it is wired to.
pub struct OrzmaTerm<B: VtBackend> {
    vt: OrzmaVt<B>,
    coalescer: Coalescer,
    pty: Pty,
}

impl<B: VtBackend> OrzmaTerm<B> {
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

    /// Spawns the login shell under a new PTY and builds the VT at the
    /// same grid size.
    pub fn spawn(options: SpawnOptions) -> OrzmaTermResult<Self> {
        let pty = Pty::spawn(&options)?;
        Ok(Self {
            vt: OrzmaVt::new(options.cols, options.rows),
            coalescer: Coalescer::default(),
            pty,
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

    /// Builds a terminal whose PTY writes land on `writer` instead of a
    /// spawned shell.
    ///
    /// A PTY is still opened at the grid size, but no child process or
    /// reader thread is started, so everything the input methods emit
    /// can be observed on `writer` — typically a
    /// [`test_support::CaptureSink`].
    pub fn detached(cols: u16, rows: u16, writer: Box<dyn Write + Send>) -> OrzmaTermResult<Self> {
        Ok(Self {
            vt: OrzmaVt::new(cols, rows),
            coalescer: Coalescer::default(),
            pty: Pty::detached(cols, rows, writer)?,
        })
    }

    ///HACK:
    /// VecでTermEventを収集しているが、この関数はほぼ米フレームで呼ばれることが予想されるため、
    /// コールバック形式などにしたほうがいい？
    pub fn pump(&mut self) -> InterpretOutput {
        while let Some(chunk) = self.pty.try_read_chunk() {
            //TODO: DamageVerdictを使い、colalescerのdeadlineを調整する。
            self.vt.interpret(&chunk);
        }
        let signals = self
            .vt
            .drain_signals()
            .map(|s| TermSignal::Vt(s))
            .collect::<Vec<_>>();
        //TODO: ChildExitの判定を行い、必要であればsignalsに追加する。
        let mut frame: Option<Frame> = None;
        if self.coalescer.is_due(Instant::now()) {
            if let Some(f) = self.vt.frame() {
                frame.replace(f);
            }
            self.coalescer.disarm();
        }
        InterpretOutput { frame, signals }
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
    /// [`OrzmaTermError::PtyResize`] and leaves the VT grid and
    /// coalescer untouched (PTY first; nothing changes on failure).
    ///
    /// A request for the grid size the VT already has reflows nothing
    /// and reports no damage, so it arms nothing either — the same gate
    /// [`Self::scroll`] applies to a clamped motion.
    pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTermResult {
        if cols == 0 || rows == 0 || Self::MAX_COLS < cols || Self::MAX_ROWS < rows {
            return Ok(());
        }
        self.pty.resize(cols, rows)?;
        if self.vt.resize(cols, rows) {
            self.coalescer.arm_or_extend(Instant::now());
        }
        Ok(())
    }

    /// Encodes a key press and writes it to the PTY.
    ///
    /// Snaps a scrolled-back viewport to the live tail first
    /// (scroll-on-input policy) so the echo is visible.
    pub fn write_key_input(
        &mut self,
        key: &TerminalKey,
        mods: &TerminalModifiers,
    ) -> OrzmaTermResult {
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
    pub fn write_mouse_input(&mut self, report: MouseReport) -> OrzmaTermResult {
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
    pub fn write_paste(&mut self, text: &str) -> OrzmaTermResult {
        if text.is_empty() {
            return Ok(());
        }
        let bracketed = self.vt.modes().bracketed_paste;
        self.snap_to_live_tail();
        self.pty
            .write_all(PtyInput::encode_paste(text, bracketed).as_bytes())
    }

    /// Snaps a scrolled-back viewport to the live tail (scroll-on-input
    /// policy), gated on [`OrzmaVt::is_at_live_tail`] so a no-op call
    /// stages no damage.
    fn snap_to_live_tail(&mut self) {
        if !self.vt.is_at_live_tail() {
            self.scroll(Scroll::Bottom);
        }
    }
}

impl<B: VtBackend + VtSelection> OrzmaTerm<B> {
    /// Anchors a new selection at an explicit viewport cell (mouse
    /// press).
    pub fn start_selection(
        &mut self,
        cell: ViewportPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> VtResult {
        self.arm_on_selection_change(|vt| vt.start_selection(cell, side, kind))
    }

    /// Anchors a new selection at the vi cursor (vi-mode `v` / `V`).
    pub fn start_selection_at_vi_cursor(&mut self, kind: SelectionKind) -> VtResult {
        self.arm_on_selection_change(|vt| vt.start_selection_at_vi_cursor(kind))
    }

    /// Moves the moving end of the active selection (mouse drag).
    pub fn update_selection(&mut self, cell: ViewportPoint, side: CellSide) -> VtResult {
        self.arm_on_selection_change(|vt| vt.update_selection(cell, side))
    }

    /// Switches selection granularity while keeping the anchor.
    pub fn change_selection_kind(&mut self, kind: SelectionKind) -> VtResult {
        self.arm_on_selection_change(|vt| vt.change_selection_kind(kind))
    }

    /// Drops any active selection.
    pub fn clear_selection(&mut self) -> VtResult {
        self.arm_on_selection_change(|vt| vt.clear_selection())
    }

    /// Applies one selection operation and arms the coalescer only
    /// when the visible selection actually changed, detected by
    /// comparing [`VtSelection::selection_range`] before and after.
    ///
    /// [`Self::scroll`] and [`Self::resize`] gate on the staged-damage
    /// `bool` the VT returns; selection cannot. The backend reports a
    /// conservative repaint for every operation that touches a live
    /// selection, so a drag sample that stays inside the cell it
    /// already covers — or a re-anchor onto the anchored cell — would
    /// arm an emit for a frame that renders identically.
    fn arm_on_selection_change(
        &mut self,
        op: impl FnOnce(&mut OrzmaVt<B>) -> VtResult<bool>,
    ) -> VtResult {
        let prev_range = self.vt.selection_range();
        op(&mut self.vt)?;
        if prev_range != self.vt.selection_range() {
            self.coalescer.arm_or_extend(Instant::now());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{CaptureSink, FailingMaster};

    fn detached_term() -> (OrzmaTerm<AlacrittyVtBackend>, CaptureSink) {
        let sink = CaptureSink::default();
        let term =
            OrzmaTerm::detached(80, 24, Box::new(sink.clone())).expect("OrzmaTerm::detached");
        (term, sink)
    }

    fn failing_term() -> OrzmaTerm<AlacrittyVtBackend> {
        OrzmaTerm {
            vt: OrzmaVt::new(80, 24),
            coalescer: Coalescer::default(),
            pty: Pty::with_master(Box::new(FailingMaster), Box::new(CaptureSink::default())),
        }
    }

    fn sizes(term: &OrzmaTerm<AlacrittyVtBackend>) -> ((u16, u16), (u16, u16)) {
        let pty = term.pty_size();
        let grid = term.vt.grid_size();
        ((pty.cols, pty.rows), (grid.cols, grid.rows))
    }

    /// Asserts that a resize reaches the VT grid, not only the PTY.
    ///
    /// Case: the user drags the window to a new size, and the renderer
    /// redraws from the grid the VT reports.
    #[test]
    fn resize_applies_the_size_to_the_vt_grid() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert_eq!(
            term.vt.grid_size(),
            GridSize {
                cols: 120,
                rows: 40
            }
        );
    }

    /// Asserts that a resize never writes through the PTY writer.
    ///
    /// Case: the user resizes the window while a program is reading
    /// stdin. The size change reaches the child as a kernel ioctl, so
    /// the decided policy is that nothing at all enters the byte
    /// stream — not even an XTWINOPS report.
    #[test]
    fn resize_does_not_write_through_the_pty_writer() {
        let (mut term, sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts that a successful resize arms the coalescer.
    ///
    /// Case: the user resizes the window at an idle shell prompt,
    /// where the reflow is the only thing that changes and no PTY
    /// output follows it.
    #[test]
    fn resize_arms_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert!(term.coalescer.is_armed());
    }

    /// Asserts that a zero-axis request leaves an already-resized
    /// terminal at its current size on both seams.
    ///
    /// Case: a minimized window, or a frame before cell metrics load,
    /// computes 0 for an axis. The decided policy is to ignore the
    /// request outright rather than clamp it to a usable size.
    #[test]
    fn a_zero_axis_resize_is_ignored() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        for (cols, rows) in [(0, 40), (120, 0), (0, 0)] {
            term.resize(cols, rows).expect("ignored resize must be Ok");
            assert_eq!(
                sizes(&term),
                ((120, 40), (120, 40)),
                "resize {cols}x{rows} must be ignored"
            );
        }
    }

    /// Asserts the per-axis cap: requests beyond `MAX_COLS` /
    /// `MAX_ROWS` are ignored, the boundary value is applied.
    ///
    /// Case: a degenerate or hostile window geometry asks for a grid
    /// far larger than any real display. The decided policy is to
    /// ignore such a request rather than clamp it, while the cap value
    /// itself stays a legal size.
    #[test]
    fn an_oversized_axis_resize_is_ignored() {
        const MAX_COLS: u16 = OrzmaTerm::<AlacrittyVtBackend>::MAX_COLS;
        const MAX_ROWS: u16 = OrzmaTerm::<AlacrittyVtBackend>::MAX_ROWS;
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
    /// The decided policy is that an ignored request is ignored on
    /// every seam, arming included.
    #[test]
    fn an_ignored_resize_does_not_arm_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(0, 40).expect("ignored resize must be Ok");
        term.resize(OrzmaTerm::<AlacrittyVtBackend>::MAX_COLS + 1, 24)
            .expect("ignored resize must be Ok");
        assert!(!term.coalescer.is_armed());
    }

    /// Asserts that a resize to the size the terminal already has arms
    /// nothing.
    ///
    /// Case: the host recomputes cells after a pixel-only window
    /// change — a DPI event, or a drag that does not cross a cell
    /// boundary — and re-applies the grid size the VT already holds.
    /// The decided policy is to schedule nothing rather than open an
    /// emit window for a reflow that did not happen.
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
    /// Case: a live window drag fires a burst of requests, and the
    /// terminal must end up at the geometry the drag settled on.
    #[test]
    fn sequential_resizes_settle_on_the_last_size() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        term.resize(90, 30).expect("resize");
        assert_eq!(sizes(&term), ((90, 30), (90, 30)));
    }

    /// Asserts PTY-first ordering via failure atomicity: when the PTY
    /// ioctl fails, the call returns `PtyResize` and the VT grid and
    /// coalescer are untouched.
    ///
    /// Case: the kernel refuses the winsize ioctl. The decided
    /// ordering is PTY first, so the renderer keeps drawing the size
    /// the child still has rather than a size the kernel never
    /// accepted.
    #[test]
    fn a_failing_pty_resize_leaves_the_vt_untouched() {
        let mut term = failing_term();
        let result = term.resize(120, 40);
        assert!(
            matches!(result, Err(OrzmaTermError::PtyResize(_))),
            "expected PtyResize, got {result:?}"
        );
        assert_eq!(term.vt.grid_size(), GridSize { cols: 80, rows: 24 });
        assert!(!term.coalescer.is_armed());
    }

    // NOTE: on the 24-row grid the first 23 newlines only fill the
    // viewport (alacritty pushes a row into history once the cursor
    // already sits on the last screen line), so `history_rows + 23`
    // lines seed exactly `history_rows`.
    fn term_with_history(history_rows: usize) -> (OrzmaTerm<AlacrittyVtBackend>, CaptureSink) {
        let (mut term, sink) = detached_term();
        let seed: Vec<u8> = (0..history_rows + 23)
            .flat_map(|i| format!("l{i}\r\n").into_bytes())
            .collect();
        term.vt_mut().interpret(&seed);
        (term, sink)
    }

    /// Asserts that `scroll` moves the viewport through the VT,
    /// cumulatively and in the requested direction.
    ///
    /// Case: the user turns the wheel back into scrollback, then
    /// forward again, and the viewport tracks each notch.
    #[test]
    fn scroll_moves_the_viewport() {
        let (mut term, _sink) = term_with_history(10);
        term.scroll(Scroll::Delta(3));
        assert_eq!(term.vt.display_offset(), DisplayOffset(3));
        term.scroll(Scroll::Delta(-2));
        assert_eq!(term.vt.display_offset(), DisplayOffset(1));
    }

    /// Asserts that a scroll which moved the viewport arms the
    /// coalescer.
    ///
    /// Case: the user scrolls into history on an idle terminal, where
    /// the viewport change is the only thing that happens and no PTY
    /// output follows it.
    #[test]
    fn scroll_arms_the_coalescer_when_the_viewport_moves() {
        let (mut term, _sink) = term_with_history(10);
        term.scroll(Scroll::Delta(3));
        assert!(term.coalescer.is_armed());
    }

    /// Asserts that a scroll which did not move the viewport arms
    /// nothing.
    ///
    /// Case: the user keeps turning the wheel after the viewport has
    /// reached the end of the scrollback, or is already pinned to the
    /// live tail. The decided policy is to schedule nothing for a
    /// motion that moved nothing.
    #[test]
    fn a_no_op_scroll_does_not_arm_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.scroll(Scroll::Delta(5));
        assert!(!term.coalescer.is_armed(), "no scrollback to move into");
        let (mut term, _sink) = term_with_history(10);
        term.scroll(Scroll::Bottom);
        assert!(!term.coalescer.is_armed(), "already at the live tail");
        term.scroll(Scroll::Delta(0));
        assert!(!term.coalescer.is_armed(), "zero delta");
    }

    /// Asserts that a no-op scroll leaves an already-open emit window's
    /// deadline untouched.
    ///
    /// Case: the user keeps spinning the wheel at the clamp while an
    /// earlier repaint is still pending.
    #[test]
    fn a_no_op_scroll_does_not_extend_the_deadline() {
        let (mut term, _sink) = term_with_history(10);
        term.scroll(Scroll::Delta(3));
        let deadline = term.coalescer.next_deadline();
        assert!(deadline.is_some(), "precondition: a real scroll arms");
        term.scroll(Scroll::Delta(0));
        assert_eq!(term.coalescer.next_deadline(), deadline);
    }

    /// Asserts that scrolling writes nothing through the PTY writer.
    ///
    /// Case: the user scrolls through history while a program is
    /// reading stdin. Viewport motion is host-side state, so the
    /// decided policy is that no bytes reach the child — neither a
    /// CSI S/T pair nor arrow keys.
    #[test]
    fn scroll_writes_nothing_through_the_pty_writer() {
        let (mut term, sink) = term_with_history(10);
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
        let (mut term, _sink) = term_with_history(10);
        term.vt_mut().scroll(Scroll::Delta(3));
        assert!(
            !term.coalescer.is_armed(),
            "precondition: nothing armed yet"
        );
        term.write_paste("x").expect("write_paste");
        assert_eq!(
            term.vt.display_offset(),
            DisplayOffset(0),
            "input must snap to the live tail"
        );
        assert!(
            term.coalescer.is_armed(),
            "the snap must schedule a repaint"
        );
    }

    /// Asserts that a detached terminal's PTY writes land on the
    /// injected sink, byte-identical.
    ///
    /// Case: a caller builds a terminal with an injected writer
    /// instead of a spawned shell, then reads back the bytes the
    /// terminal produced.
    #[test]
    fn detached_routes_writes_to_the_injected_sink() {
        let (mut term, sink) = detached_term();
        term.write_paste("hi").expect("write_paste");
        assert_eq!(sink.contents(), b"hi");
    }

    /// Asserts that `write_paste("")` writes nothing at all.
    ///
    /// Case: the user pastes with an empty clipboard. The decided
    /// policy is to write nothing at all rather than an empty
    /// bracketed-paste frame, which would still wake the receiving
    /// program.
    #[test]
    fn empty_paste_writes_nothing_to_the_pty() {
        let (mut term, sink) = detached_term();
        term.write_paste("").expect("write_paste");
        assert_eq!(sink.contents(), b"");
    }
}
