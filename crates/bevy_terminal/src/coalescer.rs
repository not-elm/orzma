//! Per-bridge frame-emit coalescer.
//!
//! Wraps the deadline state machine that decides when
//! [`crate::plugin::drain_pty_chunks`] flushes accumulated terminal
//! damage to the wire. The Coalescer never touches the `Term` directly;
//! the chunk-drain system classifies damage and passes a verdict in.

use crate::vt::damage::DamageVerdict;
use std::cmp::min;
use std::time::{Duration, Instant};

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
    pub(crate) const IDLE: Duration = Duration::from_millis(3);
    /// Hard ceiling: maximum time the first pending chunk waits.
    pub(crate) const MAX_CAP: Duration = Duration::from_millis(12);
    /// Row-count cap for the immediate-flush branch that fires on
    /// `ManyRows + pending_user_input`. NeoVim 1-line scroll in a TUI
    /// dirties scrolled-in row + status line + (sometimes) tabline =
    /// 2-3 rows; cap of 4 leaves headroom while still excluding bigger
    /// redraws (`:redraw!`, mode-line transitions) from bypassing the
    /// debounce window.
    const MANY_ROWS_INSTANT_CAP: usize = 4;

    /// Constructs a disarmed Coalescer.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns true while a window is open (deadline armed).
    pub(crate) fn is_armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Arms the window on first call after disarm; extends `last_chunk_at` on
    /// subsequent calls inside the same window.
    pub(crate) fn arm_or_extend(&mut self, now: Instant) {
        self.armed_at.get_or_insert(now);
        self.last_chunk_at = Some(now);
    }

    /// Resets the window. Called after `emit_now` runs.
    pub(crate) fn disarm(&mut self) {
        self.armed_at = None;
        self.last_chunk_at = None;
    }

    /// Immediate-flush eligibility. Called by the bridge after `state.advance`
    /// has run and damage has been classified.
    ///
    /// `pending_user_input` is consumed *by the caller* on a `true` return —
    /// the caller flips the bool before invoking this method only when the
    /// verdict warrants. This method is pure: it does not mutate state.
    ///
    /// # Invariants
    ///
    /// `DamageVerdict::Full` is deliberately NOT in the immediate-flush set.
    /// alt-screen entry (`\x1b[?1049h\x1b[2J\x1b[H`) and row contents typically
    /// arrive in separate PTY chunks 1-5 ms apart; immediate-flushing on Full
    /// would broadcast a snapshot of the post-clear, pre-content `Term` (all
    /// rows blank) before content arrives. Routing Full through the coalescer
    /// window lets the deadline-driven flush absorb the row-content chunk into
    /// the same emit.
    ///
    /// PR-E2b extends this with a `ManyRows` branch gated on a row cap and
    /// the coalescer NOT being armed — see spec § 2 and § 7.
    pub(crate) fn should_flush_immediately(
        &self,
        is_bootstrap: bool,
        verdict: &DamageVerdict,
        pending_user_input: bool,
    ) -> bool {
        if is_bootstrap {
            return true;
        }
        if !pending_user_input {
            return false;
        }
        match verdict {
            DamageVerdict::AtMostOneRow => true,
            // NOTE: the `armed_at.is_none()` guard protects post-Full coalescing —
            // if a prior Full chunk is already debouncing, its window must run to
            // completion rather than be cut short by a follow-up ManyRows chunk.
            DamageVerdict::ManyRows { rows } if *rows <= Self::MANY_ROWS_INSTANT_CAP => {
                self.armed_at.is_none()
            }
            _ => false,
        }
    }

    /// Returns the next deadline as `min(last_chunk + IDLE, armed + MAX_CAP)`.
    /// Returns `None` when the Coalescer is disarmed.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let armed = self.armed_at?;
        let last = self.last_chunk_at.unwrap_or(armed);
        Some(min(last + Self::IDLE, armed + Self::MAX_CAP))
    }
}
