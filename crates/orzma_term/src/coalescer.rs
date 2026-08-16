//! Per-terminal frame-emit coalescer.
//!
//! Owns the deadline state machine plus the flush flags that decide
//! when the owning terminal emits accumulated damage: the coalesce
//! window (idle debounce and hard cap), the noted-user-input
//! timestamp behind the immediate-flush path, and the bootstrap flag
//! behind the initial snapshot.

use orzma_vt::prelude::DamageVerdict;
use std::cmp::min;
use std::time::{Duration, Instant};

/// Coalescer state. One instance per terminal.
#[derive(Debug)]
pub struct Coalescer {
    /// Arrival time of the first chunk that opened the current coalesce
    /// window. Anchors the `MAX_CAP` hard-flush deadline. Set only by the
    /// first `arm_or_extend` call; subsequent chunks in the same window do
    /// not move it. Cleared back to `None` by `disarm`.
    armed_at: Option<Instant>,
    /// Arrival time of the most recent chunk in the current window. Anchors
    /// the `IDLE` debounce deadline. Updated on every `arm_or_extend` call so
    /// the idle timer resets whenever new input arrives. Cleared back to
    /// `None` by `disarm`.
    last_chunk_at: Option<Instant>,
    /// When user input last reached the PTY, while its echo is still
    /// unsettled. Input fresh within [`Coalescer::INPUT_ECHO_WINDOW`]
    /// qualifies echo-shaped damage for an immediate flush;
    /// [`Coalescer::settle_emit`] consumes it so one noted input buys at
    /// most one immediate flush.
    last_input_at: Option<Instant>,
    /// True until the first emit settles. The owner emits the initial
    /// snapshot without waiting for a deadline while this holds.
    bootstrap: bool,
}

impl Default for Coalescer {
    /// Builds a fresh coalescer: disarmed, no noted input, and the
    /// bootstrap emit still owed.
    fn default() -> Self {
        Self {
            armed_at: None,
            last_chunk_at: None,
            last_input_at: None,
            bootstrap: true,
        }
    }
}

/// What the owner should do with the damage a chunk just staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushDecision {
    /// Emit now, skipping the deadline.
    Now,
    /// Let the armed window's deadline drive the emit.
    Deadline,
}

impl Coalescer {
    /// Idle-debounce: time after the most recent chunk before flushing.
    const IDLE: Duration = Duration::from_millis(3);
    /// Hard ceiling: maximum time the first pending chunk waits.
    const MAX_CAP: Duration = Duration::from_millis(12);
    /// Row-count cap for the immediate-flush branch that fires on
    /// `ManyRows` with fresh input. NeoVim 1-line scroll in a TUI
    /// dirties scrolled-in row + status line + (sometimes) tabline =
    /// 2-3 rows; cap of 4 leaves headroom while still excluding bigger
    /// redraws (`:redraw!`, mode-line transitions) from bypassing the
    /// debounce window.
    const MANY_ROWS_INSTANT_CAP: usize = 4;
    /// Freshness window for noted user input. Long enough to cover a
    /// slow echo path (remote shells), short enough that an old
    /// keystroke cannot fast-flush unrelated output arriving later.
    const INPUT_ECHO_WINDOW: Duration = Duration::from_millis(150);

    /// Returns true while a window is open (deadline armed).
    #[inline]
    pub const fn is_armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Arms the window on first call after disarm; extends `last_chunk_at` on
    /// subsequent calls inside the same window.
    #[inline]
    pub fn arm_or_extend(&mut self, now: Instant) {
        self.armed_at.get_or_insert(now);
        self.last_chunk_at = Some(now);
    }

    /// Records that user input reached the PTY, qualifying its echo
    /// for an immediate flush.
    ///
    /// # Invariants
    ///
    /// Call only after the PTY write succeeded — noting a failed write
    /// leaves a phantom credit that fast-flushes unrelated output.
    #[inline]
    pub fn note_user_input(&mut self, now: Instant) {
        self.last_input_at = Some(now);
    }

    /// Records one interpreted chunk: decides whether its damage should
    /// flush immediately, then opens or extends the coalesce window.
    ///
    /// The decision is evaluated against the pre-chunk window state — the
    /// small-`ManyRows` branch requires a closed window, so arming first
    /// would make it unreachable. The window is armed regardless of the
    /// decision: an owner that does not emit on [`FlushDecision::Now`]
    /// still gets the deadline as a fallback.
    ///
    /// # Invariants
    ///
    /// `DamageVerdict::Full` is deliberately NOT in the immediate-flush set.
    /// alt-screen entry (`\x1b[?1049h\x1b[2J\x1b[H`) and row contents typically
    /// arrive in separate PTY chunks 1-5 ms apart; immediate-flushing on Full
    /// would emit a snapshot of the post-clear, pre-content grid (all rows
    /// blank) before content arrives. Routing Full through the coalescer
    /// window lets the deadline-driven flush absorb the row-content chunk
    /// into the same emit.
    pub fn observe_chunk(&mut self, now: Instant, verdict: &DamageVerdict) -> FlushDecision {
        let decision = if self.qualifies_for_immediate_flush(now, verdict) {
            FlushDecision::Now
        } else {
            FlushDecision::Deadline
        };
        self.arm_or_extend(now);
        decision
    }

    /// Returns true until the first emit settles. The owner emits the
    /// initial snapshot without waiting for a deadline while this
    /// holds.
    #[inline]
    pub fn needs_bootstrap(&self) -> bool {
        self.bootstrap
    }

    /// Settles a completed emit: closes the window, consumes the noted
    /// input, and marks the bootstrap paint done.
    ///
    /// # Invariants
    ///
    /// Call only after a frame was actually produced — settling on a
    /// decision alone would spend the immediate-flush credit and the
    /// bootstrap debt with nothing painted.
    pub fn settle_emit(&mut self) {
        self.disarm();
        self.last_input_at = None;
        self.bootstrap = false;
    }

    /// Resets the window. Serves non-consuming resets (an emit settles
    /// through [`Self::settle_emit`] instead): the bootstrap debt and
    /// any noted input survive, so a resize/scroll/selection repaint
    /// cannot spend the echo credit it did not pay.
    #[inline]
    pub fn disarm(&mut self) {
        self.armed_at = None;
        self.last_chunk_at = None;
    }

    /// Returns true while the armed window's deadline has elapsed at
    /// `now`; false when disarmed (no deadline is due).
    #[inline]
    pub fn is_due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|d| d <= now)
    }

    /// Returns the next deadline as `min(last_chunk + IDLE, armed + MAX_CAP)`.
    /// Returns `None` when the Coalescer is disarmed.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let armed = self.armed_at?;
        let last = self.last_chunk_at.unwrap_or(armed);
        Some(min(last + Self::IDLE, armed + Self::MAX_CAP))
    }

    /// Immediate-flush eligibility for one chunk's damage, evaluated
    /// against the pre-chunk window state.
    fn qualifies_for_immediate_flush(&self, now: Instant, verdict: &DamageVerdict) -> bool {
        if !self.input_is_fresh(now) {
            return false;
        }
        match verdict {
            DamageVerdict::AtMostOneRow => true,
            DamageVerdict::ManyRows { rows } if *rows <= Self::MANY_ROWS_INSTANT_CAP => {
                self.armed_at.is_none()
            }
            _ => false,
        }
    }

    /// Whether noted input lies within [`Self::INPUT_ECHO_WINDOW`] of `now`.
    fn input_is_fresh(&self, now: Instant) -> bool {
        self.last_input_at
            .is_some_and(|at| now.duration_since(at) <= Self::INPUT_ECHO_WINDOW)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Instant {
        Instant::now()
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    /// Asserts that only the first `arm_or_extend` anchors the hard
    /// cap: a later chunk in the same window leaves the `MAX_CAP`
    /// deadline unchanged.
    ///
    /// Case: a burst of output chunks lands in one coalesce window.
    #[test]
    fn arming_anchors_the_window_at_the_first_chunk() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.arm_or_extend(t0);
        coalescer.arm_or_extend(t0 + ms(10));
        assert_eq!(coalescer.next_deadline(), Some(t0 + Coalescer::MAX_CAP));
    }

    /// Asserts that each chunk inside the window moves the idle
    /// deadline to `last_chunk + IDLE` while under the cap.
    ///
    /// Case: output keeps trickling in faster than the debounce, so
    /// the flush keeps waiting for the stream to go quiet.
    #[test]
    fn each_chunk_extends_the_idle_deadline() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.arm_or_extend(t0);
        coalescer.arm_or_extend(t0 + ms(1));
        assert_eq!(
            coalescer.next_deadline(),
            Some(t0 + ms(1) + Coalescer::IDLE)
        );
    }

    /// Asserts that a busy window becomes due at the hard cap even
    /// though the most recent chunk keeps the idle deadline in the
    /// future.
    ///
    /// Case: continuous spam (`yes`) extends the debounce forever; the
    /// screen must still update by the cap.
    #[test]
    fn the_hard_cap_bounds_a_busy_window() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.arm_or_extend(t0);
        coalescer.arm_or_extend(t0 + ms(10));
        assert!(coalescer.is_due(t0 + Coalescer::MAX_CAP));
    }

    /// Asserts that a disarmed coalescer is never due, at any time.
    ///
    /// Case: an idle terminal pumped every frame.
    #[test]
    fn a_disarmed_coalescer_is_never_due() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        assert!(!coalescer.is_due(t0 + ms(60_000)));
        coalescer.arm_or_extend(t0);
        coalescer.disarm();
        assert!(!coalescer.is_due(t0 + ms(60_000)));
    }

    /// Asserts that the window becomes due exactly once the idle
    /// debounce elapses after the last chunk.
    ///
    /// Case: output stops, and the repaint fires when the stream has
    /// been quiet for the debounce.
    #[test]
    fn the_window_is_due_once_idle_elapses() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.arm_or_extend(t0);
        assert!(!coalescer.is_due(t0 + Coalescer::IDLE - Duration::from_micros(1)));
        assert!(coalescer.is_due(t0 + Coalescer::IDLE));
    }

    /// Asserts that a default-built coalescer still owes the bootstrap
    /// emit and starts disarmed.
    ///
    /// `Default` is hand-written for this; rederiving
    /// `#[derive(Default)]` would silently start `bootstrap` at
    /// `false` and the initial snapshot would never emit.
    ///
    /// Case: a freshly spawned terminal before its first pump.
    #[test]
    fn a_fresh_coalescer_needs_bootstrap() {
        let coalescer = Coalescer::default();
        assert!(coalescer.needs_bootstrap());
        assert!(!coalescer.is_armed());
    }

    /// Asserts that `settle_emit` closes the window and consumes both
    /// the bootstrap debt and the noted input.
    ///
    /// Case: the owner produced a frame and accounts for everything
    /// that frame paid off.
    #[test]
    fn settle_emit_closes_the_window_and_consumes_both_flags() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.note_user_input(t0);
        coalescer.arm_or_extend(t0);
        coalescer.settle_emit();
        assert!(!coalescer.is_armed());
        assert!(!coalescer.needs_bootstrap());
        assert_eq!(
            coalescer.observe_chunk(t0 + ms(1), &DamageVerdict::AtMostOneRow),
            FlushDecision::Deadline
        );
    }

    /// Asserts that `disarm` closes only the window: the bootstrap
    /// debt and the noted input both survive.
    ///
    /// The decided split is that `disarm` serves resets that painted
    /// nothing the flags account for; only `settle_emit` consumes
    /// them, so a resize or scroll repaint cannot spend the echo
    /// credit it did not pay.
    ///
    /// Case: a window-resize repaint lands between a keystroke and its
    /// slow echo.
    #[test]
    fn disarm_preserves_bootstrap_and_noted_input() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.note_user_input(t0);
        coalescer.arm_or_extend(t0);
        coalescer.disarm();
        assert!(coalescer.needs_bootstrap());
        assert_eq!(
            coalescer.observe_chunk(t0 + ms(2), &DamageVerdict::AtMostOneRow),
            FlushDecision::Now
        );
    }

    /// Asserts that echo-shaped damage after fresh input flushes
    /// immediately.
    ///
    /// Case: a keystroke at a shell prompt echoes into a single row.
    #[test]
    fn an_echo_sized_chunk_after_input_flushes_now() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.note_user_input(t0);
        assert_eq!(
            coalescer.observe_chunk(t0 + ms(1), &DamageVerdict::AtMostOneRow),
            FlushDecision::Now
        );
    }

    /// Asserts that one-row damage without noted input waits for the
    /// deadline.
    ///
    /// Case: a background program trickles a log line while the user
    /// is not typing.
    #[test]
    fn an_echo_sized_chunk_without_input_waits() {
        let mut coalescer = Coalescer::default();
        assert_eq!(
            coalescer.observe_chunk(base(), &DamageVerdict::AtMostOneRow),
            FlushDecision::Deadline
        );
    }

    /// Asserts that input older than the echo window no longer
    /// qualifies damage for an immediate flush.
    ///
    /// Case: the user pressed Enter on a long-running command whose
    /// output arrives much later.
    #[test]
    fn stale_input_does_not_flush_now() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.note_user_input(t0);
        assert_eq!(
            coalescer.observe_chunk(
                t0 + Coalescer::INPUT_ECHO_WINDOW + ms(1),
                &DamageVerdict::AtMostOneRow
            ),
            FlushDecision::Deadline
        );
    }

    /// Asserts that a small `ManyRows` repaint flushes immediately
    /// only while the window is closed, and that the decision is
    /// evaluated before the chunk arms the window.
    ///
    /// Case: a NeoVim keystroke dirties 2-3 rows on a quiet terminal,
    /// versus the same rows arriving in the middle of a coalescing
    /// burst.
    #[test]
    fn a_small_tui_repaint_flushes_now_only_when_the_window_is_closed() {
        let t0 = base();
        let mut quiet = Coalescer::default();
        quiet.note_user_input(t0);
        assert_eq!(
            quiet.observe_chunk(t0 + ms(1), &DamageVerdict::ManyRows { rows: 3 }),
            FlushDecision::Now
        );

        let mut busy = Coalescer::default();
        busy.note_user_input(t0);
        busy.arm_or_extend(t0);
        assert_eq!(
            busy.observe_chunk(t0 + ms(1), &DamageVerdict::ManyRows { rows: 3 }),
            FlushDecision::Deadline
        );
    }

    /// Asserts the `ManyRows` cap boundary: 4 rows still flush
    /// immediately, 5 rows wait.
    ///
    /// Case: the cap is sized for a TUI keystroke (scrolled-in row +
    /// status line + tabline, plus one row of headroom); anything
    /// larger reads as a bulk redraw.
    #[test]
    fn the_many_rows_cap_boundary() {
        let t0 = base();
        let mut at_cap = Coalescer::default();
        at_cap.note_user_input(t0);
        assert_eq!(
            at_cap.observe_chunk(
                t0 + ms(1),
                &DamageVerdict::ManyRows {
                    rows: Coalescer::MANY_ROWS_INSTANT_CAP
                }
            ),
            FlushDecision::Now
        );

        let mut over_cap = Coalescer::default();
        over_cap.note_user_input(t0);
        assert_eq!(
            over_cap.observe_chunk(
                t0 + ms(1),
                &DamageVerdict::ManyRows {
                    rows: Coalescer::MANY_ROWS_INSTANT_CAP + 1
                }
            ),
            FlushDecision::Deadline
        );
    }

    /// Asserts that `Full` and `Idle` damage never flush immediately,
    /// even with fresh input.
    ///
    /// `Full` is excluded because alt-screen entry delivers its clear
    /// and its row contents in separate chunks a few milliseconds
    /// apart; an immediate flush would emit the blank in-between
    /// grid. `Idle` has nothing to paint.
    ///
    /// Case: the user presses Enter on `vim`, whose startup clears the
    /// screen before drawing.
    #[test]
    fn full_and_idle_never_flush_now() {
        let t0 = base();
        for verdict in [DamageVerdict::Full, DamageVerdict::Idle] {
            let mut coalescer = Coalescer::default();
            coalescer.note_user_input(t0);
            assert_eq!(
                coalescer.observe_chunk(t0 + ms(1), &verdict),
                FlushDecision::Deadline,
                "{verdict:?} must wait for the deadline"
            );
        }
    }

    /// Asserts that `observe_chunk` arms the window regardless of the
    /// decision it returns.
    ///
    /// Arming even on [`FlushDecision::Now`] is the decided fallback:
    /// an owner that fails to emit immediately still gets the deadline
    /// repaint instead of losing the chunk.
    ///
    /// Case: every interpreted chunk, echo-shaped or not, opens the
    /// coalesce window.
    #[test]
    fn observe_chunk_always_arms() {
        let t0 = base();
        let mut idle = Coalescer::default();
        idle.observe_chunk(t0, &DamageVerdict::Idle);
        assert!(idle.is_armed());

        let mut echo = Coalescer::default();
        echo.note_user_input(t0);
        assert_eq!(
            echo.observe_chunk(t0 + ms(1), &DamageVerdict::AtMostOneRow),
            FlushDecision::Now
        );
        assert!(echo.is_armed());
    }

    /// Asserts that one noted input buys at most one immediate flush:
    /// after the emit settles, the next echo-shaped chunk waits.
    ///
    /// Case: a keystroke's echo arrives as two trickle chunks; the
    /// first paints immediately and the second coalesces instead of
    /// emitting per chunk.
    #[test]
    fn only_one_immediate_flush_per_noted_input() {
        let mut coalescer = Coalescer::default();
        let t0 = base();
        coalescer.note_user_input(t0);
        assert_eq!(
            coalescer.observe_chunk(t0 + ms(1), &DamageVerdict::AtMostOneRow),
            FlushDecision::Now
        );
        coalescer.settle_emit();
        assert_eq!(
            coalescer.observe_chunk(t0 + ms(2), &DamageVerdict::AtMostOneRow),
            FlushDecision::Deadline
        );
    }
}
