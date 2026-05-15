//! Lifecycle state machine for the shared Chromium process.
//!
//! Transitions:
//! ```text
//! Stopped         --attach()-->            Starting          (AttachOutcome::MustLaunch)
//! Starting        --attach()-->            Starting          (AttachOutcome::Wait)
//! Starting        --mark_started()-->      Running { 1 }
//! Running { n }   --attach()-->            Running { n+1 }   (AttachOutcome::Reused)
//! Running { n>1 } --detach()-->            Running { n-1 }
//! Running { 1 }   --detach()-->            StoppingAfter(now + GRACE)
//! StoppingAfter   --attach()-->            Running { 1 }     (AttachOutcome::Reused)
//! StoppingAfter   --grace_elapsed()(true)->Stopped
//! ```
//!
//! All transitions happen inside one critical section above this type
//! (`BrowserService` wraps it in `Arc<Mutex<ChromiumState>>`), so the
//! refcount and launch/kill decisions are atomic.

use std::time::{Duration, Instant};

/// Current state of the shared Chromium process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    /// No process running. Next `attach()` starts one.
    Stopped,
    /// Process is currently launching; subsequent `attach()` callers wait.
    Starting,
    /// Process is up; `pages` is the number of attached browser activities.
    Running {
        /// Number of attached pages (≥ 1).
        pages: u32,
    },
    /// All pages detached at the embedded `Instant`; if no `attach()` arrives
    /// before then, `grace_elapsed()` returns true and the process is torn down.
    StoppingAfter(Instant),
}

/// Result of an `attach()` call.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AttachOutcome {
    /// Caller must launch Chromium; call `mark_started` when ready.
    MustLaunch,
    /// Caller must wait for an existing launch (somebody else is `Starting`).
    Wait,
    /// Reused an existing `Running` or cancelled-grace process.
    Reused,
}

/// State machine. Owned by `BrowserService` behind a `Mutex` (or equivalent
/// synchronization primitive); not internally thread-safe.
pub(crate) struct ChromiumState {
    phase: Phase,
    grace: Duration,
}

// NOTE: methods are unused until BrowserService wires this up in Task 2.8.
#[cfg_attr(not(test), allow(dead_code))]
impl ChromiumState {
    /// Create a fresh state machine with the given idle-grace duration.
    pub(crate) fn new(grace: Duration) -> Self {
        Self {
            phase: Phase::Stopped,
            grace,
        }
    }

    /// Snapshot the current phase. Used by tests and the lifecycle
    /// coordinator to decide what to do next.
    pub(crate) fn snapshot(&self) -> Phase {
        self.phase.clone()
    }

    /// Increment the page refcount (or kick off a launch if `Stopped` /
    /// re-enter `Running` from `StoppingAfter`). Returns what the caller must do.
    pub(crate) fn attach(&mut self) -> AttachOutcome {
        match self.phase {
            Phase::Stopped => {
                self.phase = Phase::Starting;
                AttachOutcome::MustLaunch
            }
            Phase::Starting => AttachOutcome::Wait,
            Phase::Running { pages } => {
                self.phase = Phase::Running { pages: pages + 1 };
                AttachOutcome::Reused
            }
            Phase::StoppingAfter(_) => {
                self.phase = Phase::Running { pages: 1 };
                AttachOutcome::Reused
            }
        }
    }

    /// Transition from `Starting` to `Running { pages: 1 }`. Idempotent:
    /// calling outside the `Starting` phase is a no-op.
    pub(crate) fn mark_started(&mut self) {
        if matches!(self.phase, Phase::Starting) {
            self.phase = Phase::Running { pages: 1 };
        }
    }

    /// Decrement the page refcount. At `pages == 0` we transition to
    /// `StoppingAfter(now + grace)`. Calling outside `Running` is a no-op.
    pub(crate) fn detach(&mut self) {
        if let Phase::Running { pages } = self.phase {
            if pages > 1 {
                self.phase = Phase::Running { pages: pages - 1 };
            } else {
                self.phase = Phase::StoppingAfter(Instant::now() + self.grace);
            }
        }
    }

    /// If we are in `StoppingAfter` and the deadline has passed, transition
    /// to `Stopped` and return `true` (the caller should now kill Chromium).
    /// Otherwise returns `false` and the phase is unchanged. Idempotent.
    pub(crate) fn grace_elapsed(&mut self) -> bool {
        if let Phase::StoppingAfter(when) = self.phase
            && Instant::now() >= when
        {
            self.phase = Phase::Stopped;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fresh() -> ChromiumState {
        ChromiumState::new(Duration::from_secs(30))
    }

    #[test]
    fn fresh_starts_in_stopped() {
        let s = fresh();
        assert_eq!(s.snapshot(), Phase::Stopped);
    }

    #[test]
    fn first_attach_requests_launch() {
        let mut s = fresh();
        assert_eq!(s.attach(), AttachOutcome::MustLaunch);
        assert_eq!(s.snapshot(), Phase::Starting);
    }

    #[test]
    fn concurrent_attach_while_starting_waits() {
        let mut s = fresh();
        s.attach();
        assert_eq!(s.attach(), AttachOutcome::Wait);
        assert_eq!(s.snapshot(), Phase::Starting);
    }

    #[test]
    fn mark_started_transitions_to_running_one() {
        let mut s = fresh();
        s.attach();
        s.mark_started();
        assert_eq!(s.snapshot(), Phase::Running { pages: 1 });
    }

    #[test]
    fn mark_started_is_noop_when_not_starting() {
        let mut s = fresh();
        s.mark_started();
        assert_eq!(s.snapshot(), Phase::Stopped);
    }

    #[test]
    fn second_attach_when_running_increments_pages() {
        let mut s = fresh();
        s.attach();
        s.mark_started();
        assert_eq!(s.attach(), AttachOutcome::Reused);
        assert_eq!(s.snapshot(), Phase::Running { pages: 2 });
    }

    #[test]
    fn detach_above_one_page_decrements() {
        let mut s = fresh();
        s.attach();
        s.mark_started();
        s.attach();
        s.detach();
        assert_eq!(s.snapshot(), Phase::Running { pages: 1 });
    }

    #[test]
    fn detach_at_one_page_schedules_grace() {
        let mut s = fresh();
        s.attach();
        s.mark_started();
        s.detach();
        match s.snapshot() {
            Phase::StoppingAfter(when) => {
                assert!(when > Instant::now());
            }
            other => panic!("expected StoppingAfter, got {other:?}"),
        }
    }

    #[test]
    fn attach_during_grace_cancels_shutdown() {
        let mut s = fresh();
        s.attach();
        s.mark_started();
        s.detach();
        assert!(matches!(s.snapshot(), Phase::StoppingAfter(_)));
        assert_eq!(s.attach(), AttachOutcome::Reused);
        assert_eq!(s.snapshot(), Phase::Running { pages: 1 });
    }

    #[test]
    fn grace_elapsed_returns_true_after_deadline() {
        let mut s = ChromiumState::new(Duration::from_millis(0));
        s.attach();
        s.mark_started();
        s.detach();
        // Zero-duration grace means deadline is `now` or earlier.
        std::thread::sleep(Duration::from_millis(1));
        assert!(s.grace_elapsed());
        assert_eq!(s.snapshot(), Phase::Stopped);
    }

    #[test]
    fn grace_elapsed_returns_false_before_deadline() {
        let mut s = ChromiumState::new(Duration::from_secs(60));
        s.attach();
        s.mark_started();
        s.detach();
        assert!(!s.grace_elapsed());
        assert!(matches!(s.snapshot(), Phase::StoppingAfter(_)));
    }

    #[test]
    fn grace_elapsed_is_noop_outside_stopping_after() {
        let mut s = fresh();
        s.attach();
        s.mark_started();
        // Currently Running; grace_elapsed should not change state.
        assert!(!s.grace_elapsed());
        assert_eq!(s.snapshot(), Phase::Running { pages: 1 });
    }

    #[test]
    fn detach_below_zero_pages_is_noop() {
        // Without any attach, detach() shouldn't crash. Defensive.
        let mut s = fresh();
        s.detach();
        assert_eq!(s.snapshot(), Phase::Stopped);
    }
}
