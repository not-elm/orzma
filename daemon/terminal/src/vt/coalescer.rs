//! Per-bridge frame-emit coalescer.
//!
//! Wraps the deadline state machine that decides when [`crate::vt::bridge::run_bridge_task`]
//! flushes accumulated terminal damage to the wire. The Coalescer never touches
//! the `Term` directly; the bridge classifies damage and passes a verdict in.

use std::cmp::min;
use std::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep_until;

/// Classification of accumulated damage that drives the immediate-flush decision.
/// The bridge constructs this once per pre-emit decision (via `Term::damage()`)
/// and reuses it for the actual emit so `Term::damage()` is never called twice
/// without an intervening `reset_damage()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageVerdict {
    /// Entire screen damaged (resize, clear, alt-screen swap).
    Full,
    /// At most one row is dirty (interactive echo / cursor-only motion).
    AtMostOneRow,
    /// Many rows dirty — must be coalesced.
    ManyRows,
    /// No rows dirty and cursor unchanged.
    Idle,
}

/// Coalescer state. One instance per bridge task.
#[derive(Debug, Default)]
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
}

impl Coalescer {
    /// Idle-debounce: time after the most recent chunk before flushing.
    pub const IDLE: Duration = Duration::from_millis(3);
    /// Hard ceiling: maximum time the first pending chunk waits.
    pub const MAX_CAP: Duration = Duration::from_millis(12);

    /// Constructs a disarmed Coalescer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true while a window is open (deadline armed).
    pub fn is_armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Arms the window on first call after disarm; extends `last_chunk_at` on
    /// subsequent calls inside the same window.
    pub fn arm_or_extend(&mut self, now: Instant) {
        self.armed_at.get_or_insert(now);
        self.last_chunk_at = Some(now);
    }

    /// Resets the window. Called after `emit_now` runs.
    pub fn disarm(&mut self) {
        self.armed_at = None;
        self.last_chunk_at = None;
    }

    /// Future for `tokio::select!`. Resolves when the deadline elapses.
    /// When disarmed, returns a future that never resolves (`future::pending`).
    pub async fn wait_deadline(&self) {
        match self.next_deadline() {
            Some(deadline) => sleep_until(deadline.into()).await,
            None => std::future::pending::<()>().await,
        }
    }

    /// Immediate-flush eligibility. Called by the bridge after `state.advance`
    /// has run and damage has been classified.
    ///
    /// `pending_user_input` is consumed *by the caller* on a `true` return —
    /// the caller flips the bool before invoking this method only when the
    /// verdict is `AtMostOneRow`. This method is pure: it does not mutate state.
    ///
    /// # Invariants
    ///
    /// `DamageVerdict::Full` is deliberately NOT in the immediate-flush set.
    /// alt-screen entry (`\x1b[?1049h\x1b[2J\x1b[H`) and row contents typically
    /// arrive in separate PTY chunks 1-5 ms apart; immediate-flushing on Full
    /// would broadcast a snapshot of the post-clear, pre-content `Term` (all
    /// rows blank) before content arrives. Routing Full through the coalescer
    /// window lets `wait_deadline`'s `try_recv` drain absorb the row-content
    /// chunk into the same emit.
    pub fn should_flush_immediately(
        &self,
        is_bootstrap: bool,
        verdict: &DamageVerdict,
        pending_user_input: bool,
    ) -> bool {
        if is_bootstrap {
            return true;
        }
        if pending_user_input && matches!(verdict, DamageVerdict::AtMostOneRow) {
            return true;
        }
        false
    }

    /// Returns the next deadline as `min(last_chunk + IDLE, armed + MAX_CAP)`.
    /// Returns `None` when the Coalescer is disarmed.
    fn next_deadline(&self) -> Option<Instant> {
        let armed = self.armed_at?;
        let last = self.last_chunk_at.unwrap_or(armed);
        Some(min(last + Self::IDLE, armed + Self::MAX_CAP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_coalescer_is_disarmed() {
        let c = Coalescer::new();
        assert!(!c.is_armed());
        assert!(c.next_deadline().is_none());
    }

    #[test]
    fn arm_or_extend_sets_armed_at_only_on_first_call() {
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.arm_or_extend(t0);
        let armed = c.armed_at;
        let t1 = t0 + Duration::from_millis(5);
        c.arm_or_extend(t1);
        assert_eq!(c.armed_at, armed, "armed_at must not move after first call");
        assert_eq!(c.last_chunk_at, Some(t1));
    }

    #[test]
    fn disarm_clears_both_fields() {
        let mut c = Coalescer::new();
        c.arm_or_extend(Instant::now());
        c.disarm();
        assert!(!c.is_armed());
        assert!(c.last_chunk_at.is_none());
    }

    #[test]
    fn next_deadline_returns_idle_when_last_chunk_recent() {
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.arm_or_extend(t0);
        // single chunk, idle and max-cap are measured from the same t0
        assert_eq!(c.next_deadline(), Some(t0 + Coalescer::IDLE));
    }

    #[test]
    fn next_deadline_caps_at_max_cap_when_idle_would_overshoot() {
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.arm_or_extend(t0);
        // last chunk arrives 10ms in — idle would be t0+10+3=13ms, but cap is t0+12.
        let t1 = t0 + Duration::from_millis(10);
        c.arm_or_extend(t1);
        assert_eq!(c.next_deadline(), Some(t0 + Coalescer::MAX_CAP));
    }

    #[test]
    fn should_flush_immediately_on_bootstrap() {
        let c = Coalescer::new();
        assert!(c.should_flush_immediately(true, &DamageVerdict::Idle, false));
        assert!(c.should_flush_immediately(true, &DamageVerdict::ManyRows, false));
    }

    #[test]
    fn should_not_flush_on_full_damage_alone() {
        // Full damage routes through the coalescer window — see Invariants
        // section on `should_flush_immediately`. The chunk-split alt-screen
        // entry case relies on this so row-content chunks arriving within
        // the window get folded into the same snapshot.
        let c = Coalescer::new();
        assert!(!c.should_flush_immediately(false, &DamageVerdict::Full, false));
        assert!(!c.should_flush_immediately(false, &DamageVerdict::Full, true));
    }

    #[test]
    fn should_flush_immediately_on_user_input_with_small_damage() {
        let c = Coalescer::new();
        assert!(c.should_flush_immediately(false, &DamageVerdict::AtMostOneRow, true));
    }

    #[test]
    fn should_not_flush_user_input_with_many_rows() {
        let c = Coalescer::new();
        assert!(!c.should_flush_immediately(false, &DamageVerdict::ManyRows, true));
    }

    #[test]
    fn should_not_flush_idle_steady_state() {
        let c = Coalescer::new();
        assert!(!c.should_flush_immediately(false, &DamageVerdict::Idle, false));
        assert!(!c.should_flush_immediately(false, &DamageVerdict::AtMostOneRow, false));
    }
}
