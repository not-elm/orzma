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

/// A live terminal: the VT emulation plus the PTY it is wired to.
pub struct OrzmaTerm<V: OrzmaVt> {
    vt: V,
    coalescer: Coalescer,
    pty: Pty,
}

impl<V: OrzmaVt> OrzmaTerm<V> {
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
        let vt = V::new(options.cols, options.rows);
        let pty = Pty::spawn(&options)?;
        Ok(Self {
            vt,
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
            vt: V::new(cols, rows),
            coalescer: Coalescer::default(),
            pty: Pty::detached(cols, rows, writer)?,
        })
    }

    ///HACK:
    /// VecでTermEventを収集しているが、この関数はほぼ米フレームで呼ばれることが予想されるため、
    /// コールバック形式などにしたほうがいい？
    pub fn pump(&mut self) -> Vec<TermSignal> {
        todo!("OrzmaTerm::pump")
    }

    /// Scrolls the grid.
    pub fn scroll(&mut self, scroll: Scroll) {
        let prev_offset = self.vt.display_offset();
        self.vt.scroll(scroll);
        if prev_offset != self.vt.display_offset() {
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
    pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTermResult {
        if cols == 0 || rows == 0 || Self::MAX_COLS < cols || Self::MAX_ROWS < rows {
            return Ok(());
        }
        self.pty.resize(cols, rows)?;
        self.vt.resize(cols, rows);
        self.coalescer.arm_or_extend(Instant::now());
        Ok(())
    }

    #[inline]
    pub const fn vt_mut(&mut self) -> &mut V {
        &mut self.vt
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
    /// policy), gated on [`OrzmaVt::at_scroll_bottom`] so a no-op call
    /// stages no damage.
    fn snap_to_live_tail(&mut self) {
        if !self.vt.at_scroll_bottom() {
            self.scroll(Scroll::Bottom);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{CaptureSink, FailingMaster};

    fn detached_term() -> (OrzmaTerm<AlacrittyVt>, CaptureSink) {
        let sink = CaptureSink::default();
        let term =
            OrzmaTerm::detached(80, 24, Box::new(sink.clone())).expect("OrzmaTerm::detached");
        (term, sink)
    }

    fn failing_term() -> OrzmaTerm<AlacrittyVt> {
        OrzmaTerm {
            vt: AlacrittyVt::new(80, 24),
            coalescer: Coalescer::default(),
            pty: Pty::with_master(Box::new(FailingMaster), Box::new(CaptureSink::default())),
        }
    }

    fn sizes(term: &OrzmaTerm<AlacrittyVt>) -> ((u16, u16), (u16, u16)) {
        let pty = term.pty_size();
        ((pty.cols, pty.rows), term.vt.grid_size())
    }

    /// Asserts that a resize reaches the VT grid, not only the PTY.
    ///
    /// Case: the renderer draws whatever the VT reports. An
    /// implementation that only performs the ioctl leaves the emulation
    /// (and therefore the rendered grid) at the stale size while the
    /// child already reflows to the new one — a desync the PTY-side
    /// test cannot see, which is why the two seams are pinned
    /// separately.
    #[test]
    fn resize_applies_the_size_to_the_vt_grid() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert_eq!(term.vt.grid_size(), (120, 40));
    }

    /// Asserts that a resize never writes through the PTY writer.
    ///
    /// Case: resize is an ioctl on the master, not stream traffic. An
    /// implementation that "resizes" by writing escape sequences (e.g.
    /// XTWINOPS) through the writer would inject bytes into the child's
    /// stdin. The assertion covers the injected writer seam — the same
    /// path every `write_*` method uses.
    #[test]
    fn resize_does_not_write_through_the_pty_writer() {
        let (mut term, sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts that a successful resize arms the coalescer.
    ///
    /// Case: a resize reflows the whole grid, but no PTY output need
    /// arrive afterwards — an idle shell prompt stays idle. Unless the
    /// resize itself opens an emit window, the repaint waits for the
    /// next unrelated chunk and the user stares at a stale grid (the
    /// predecessor documented exactly this trap and armed on resize).
    #[test]
    fn resize_arms_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(120, 40).expect("resize");
        assert!(term.coalescer.is_armed());
    }

    /// Asserts that a zero-axis request leaves an already-resized
    /// terminal at its current size on both seams.
    ///
    /// Case: a minimized window or a pre-metrics frame computes 0 for
    /// an axis; the decided policy is ignore, not clamp. The fixture is
    /// first resized away from the constructor default so that "stays
    /// unchanged" is distinguishable from "was reset to the initial
    /// size".
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
    /// Case: the zero guard alone accepts 65535x65535 — a
    /// multi-billion-cell VT allocation issued after the PTY was
    /// already resized, i.e. an OOM/hang with the two sides desynced
    /// (the cap rationale lives on the constants). Pinned at the
    /// boundary without actually allocating a huge grid.
    #[test]
    fn an_oversized_axis_resize_is_ignored() {
        const MAX_COLS: u16 = OrzmaTerm::<AlacrittyVt>::MAX_COLS;
        const MAX_ROWS: u16 = OrzmaTerm::<AlacrittyVt>::MAX_ROWS;
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
    /// Case: ignored means fully ignored — arming without staging any
    /// damage would schedule an emit deadline for a repaint that never
    /// comes, waking the emit path for nothing on every minimized-
    /// window frame.
    #[test]
    fn an_ignored_resize_does_not_arm_the_coalescer() {
        let (mut term, _sink) = detached_term();
        term.resize(0, 40).expect("ignored resize must be Ok");
        term.resize(OrzmaTerm::<AlacrittyVt>::MAX_COLS + 1, 24)
            .expect("ignored resize must be Ok");
        assert!(!term.coalescer.is_armed());
    }

    /// Asserts that back-to-back resizes settle on the last requested
    /// size on both seams.
    ///
    /// Case: a live window drag fires a burst of requests. Any
    /// caching or short-circuit mistake that latches onto an earlier
    /// size leaves the terminal permanently mis-sized relative to the
    /// final window geometry.
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
    /// Case: the ordering contract is deliberately the reverse of the
    /// predecessor (which resized the VT before the PTY): on ioctl
    /// failure the renderer must keep drawing the size the child still
    /// has, never a size the kernel refused. Success-path tests cannot
    /// distinguish the two orderings — only injected failure can, so
    /// this test is the sole guard against a silent VT-first
    /// regression.
    #[test]
    fn a_failing_pty_resize_leaves_the_vt_untouched() {
        let mut term = failing_term();
        let result = term.resize(120, 40);
        assert!(
            matches!(result, Err(OrzmaTermError::PtyResize(_))),
            "expected PtyResize, got {result:?}"
        );
        assert_eq!(term.vt.grid_size(), (80, 24));
        assert!(!term.coalescer.is_armed());
    }

    // NOTE: on the 24-row grid the first 23 newlines only fill the
    // viewport (alacritty pushes a row into history once the cursor
    // already sits on the last screen line), so `history_rows + 23`
    // lines seed exactly `history_rows`.
    fn term_with_history(history_rows: usize) -> (OrzmaTerm<AlacrittyVt>, CaptureSink) {
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
    /// Case: the wheel/vi path once `apply_scroll` wires up — the
    /// motion arithmetic is pinned at the VT layer, so this pins only
    /// that `OrzmaTerm::scroll` actually delegates. A method that
    /// swallowed the motion (or inverted it) would leave every consumer
    /// scrolling nothing.
    #[test]
    fn scroll_moves_the_viewport() {
        let (mut term, _sink) = term_with_history(10);
        term.scroll(Scroll::Delta(3));
        assert_eq!(term.vt.display_offset(), 3);
        term.scroll(Scroll::Delta(-2));
        assert_eq!(term.vt.display_offset(), 1);
    }

    /// Asserts that a scroll which moved the viewport arms the
    /// coalescer.
    ///
    /// Case: a scroll changes what is visible but produces no PTY
    /// output, so nothing else opens an emit window — without arming,
    /// the view stays stale until the next unrelated chunk (the same
    /// trap as resize; the predecessor armed on every scroll op).
    #[test]
    fn scroll_arms_the_coalescer_when_the_viewport_moves() {
        let (mut term, _sink) = term_with_history(10);
        term.scroll(Scroll::Delta(3));
        assert!(term.coalescer.is_armed());
    }

    /// Asserts that a scroll which did not move the viewport arms
    /// nothing.
    ///
    /// Case: no visual change means no repaint to schedule — arming
    /// anyway would open an emit window for a repaint that never
    /// comes on every wheel notch at the clamp (mirrors
    /// `an_ignored_resize_does_not_arm_the_coalescer`).
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
    /// Case: the blind spot of `is_armed` — routing a no-op through
    /// `arm_or_extend` keeps `is_armed()` true but bumps
    /// `last_chunk_at`, sliding the IDLE deadline and delaying the
    /// flush of whatever is already pending. Comparing
    /// `next_deadline()` before and after catches that.
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
    /// Case: viewport motion is host-side state; an implementation that
    /// "scrolls" by writing CSI S/T or arrow-key sequences would inject
    /// bytes into the child's stdin. (The alternate-scroll wheel→arrow
    /// conversion is the input layer's job, not this method's.)
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
    /// Case: the user scrolls into history, then types or pastes. The
    /// echo will arrive as PTY output, but the snap itself is a
    /// host-side viewport change — if `snap_to_live_tail` bypasses the
    /// arming path, the view teleports without a scheduled emit and the
    /// screen shows stale history until the echo happens to arrive
    /// (the predecessor routed key input through its arming scroll
    /// path). The scroll-back is seeded through `vt_mut()` so the
    /// paste is the only arm candidate.
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
            0,
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
    /// Case: the test-support seam itself — every downstream test that
    /// asserts "these bytes reached the PTY" (e.g. `bevy_orzma_term`'s
    /// request-observer tests) trusts `detached` to route the write
    /// seam into the sink. A regression here silently turns all of
    /// those assertions into checks against an unrelated buffer.
    #[test]
    fn detached_routes_writes_to_the_injected_sink() {
        let (mut term, sink) = detached_term();
        term.write_paste("hi").expect("write_paste");
        assert_eq!(sink.contents(), b"hi");
    }

    /// Asserts that `write_paste("")` writes nothing at all.
    ///
    /// Case: an empty clipboard paste. The no-op contract lives here in
    /// `write_paste` (documented early return): nothing may reach the
    /// PTY — in bracketed-paste mode even an empty frame would wake the
    /// receiving app.
    #[test]
    fn empty_paste_writes_nothing_to_the_pty() {
        let (mut term, sink) = detached_term();
        term.write_paste("").expect("write_paste");
        assert_eq!(sink.contents(), b"");
    }
}
