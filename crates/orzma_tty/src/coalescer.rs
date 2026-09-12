//! Per-terminal frame-emit coalescer: the coalesce window (idle debounce
//! and hard cap) and the bootstrap flag that decide when the owning
//! terminal emits accumulated damage.

use std::cmp::min;
use std::time::{Duration, Instant};

/// Coalescer state. One instance per terminal.
#[derive(Debug)]
pub struct Coalescer {
    /// Arrival time of the first chunk that opened the current coalesce
    /// window, anchoring the `MAX_CAP` hard-flush deadline; `None` while
    /// disarmed. Later chunks in the same window do not move it.
    armed_at: Option<Instant>,
    /// Arrival time of the most recent chunk in the current window,
    /// anchoring the `IDLE` debounce deadline; `None` while disarmed.
    /// Every chunk moves it, so the idle timer resets on new input.
    last_chunk_at: Option<Instant>,
    /// True until the first emit settles. The owner emits the initial
    /// snapshot without waiting for a deadline while this holds.
    bootstrap: bool,
}

impl Default for Coalescer {
    /// Builds a fresh coalescer: disarmed, with the bootstrap emit
    /// still owed.
    fn default() -> Self {
        Self {
            armed_at: None,
            last_chunk_at: None,
            bootstrap: true,
        }
    }
}

impl Coalescer {
    /// Idle-debounce: time after the most recent chunk before flushing.
    const IDLE: Duration = Duration::from_millis(3);
    /// Hard ceiling: maximum time the first pending chunk waits.
    const MAX_CAP: Duration = Duration::from_millis(12);

    /// Returns true while a window is open (deadline armed).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "only the coalescer and pump tests read the armed state directly"
        )
    )]
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

    /// Returns true until the first emit settles. The owner emits the
    /// initial snapshot without waiting for a deadline while this
    /// holds.
    #[inline]
    pub fn needs_bootstrap(&self) -> bool {
        self.bootstrap
    }

    /// Settles a completed emit: closes the window and marks the bootstrap
    /// paint done.
    ///
    /// Call it only after a frame was actually produced.
    pub fn settle_emit(&mut self) {
        self.disarm();
        self.bootstrap = false;
    }

    /// Resets the window without touching the bootstrap debt; a completed
    /// emit settles through [`Self::settle_emit`] instead.
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
    /// Case: output keeps trickling in faster than the debounce.
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
    /// Case: continuous output from `yes` keeps extending the debounce.
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
    /// Case: the host pumps an idle terminal every frame.
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
    /// Case: a program prints one chunk of output and then goes quiet.
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
    /// Case: a terminal has just spawned and has not been pumped yet.
    #[test]
    fn a_fresh_coalescer_needs_bootstrap() {
        let coalescer = Coalescer::default();
        assert!(coalescer.needs_bootstrap());
        assert!(!coalescer.is_armed());
    }

    /// Asserts that settling a completed emit closes the coalesce window
    /// and clears the bootstrap debt.
    ///
    /// Case: the terminal paints its initial snapshot.
    #[test]
    fn settle_emit_closes_the_window_and_clears_the_bootstrap_debt() {
        let t0 = base();
        let mut coalescer = Coalescer::default();
        coalescer.arm_or_extend(t0);
        assert!(coalescer.is_armed());
        assert!(coalescer.needs_bootstrap());

        coalescer.settle_emit();
        assert!(!coalescer.is_armed());
        assert!(!coalescer.needs_bootstrap());
    }

    /// Asserts that disarming closes the window without clearing the
    /// bootstrap debt.
    ///
    /// Case: a resize discards the staged damage before the initial
    /// snapshot has ever been painted.
    #[test]
    fn disarm_closes_the_window_but_keeps_the_bootstrap_debt() {
        let t0 = base();
        let mut coalescer = Coalescer::default();
        coalescer.arm_or_extend(t0);

        coalescer.disarm();
        assert!(!coalescer.is_armed());
        assert!(coalescer.needs_bootstrap());
    }
}
