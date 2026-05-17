//! Test-only inspection helpers for [`TerminalService`].
//!
//! Compiled under `cfg(test)` or the `test-helpers` feature. Not part of
//! the production surface; consumers are the in-crate unit tests and the
//! integration tests under `daemon/terminal/tests/`.

use crate::error::TerminalResult;
use crate::service::TerminalService;
use crate::vt::frame_ring::WireMessage;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{TermDamage, TermMode};
use bytes::Bytes;
use ozmux_multiplexer::ActivityId;
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast, mpsc};

/// Snapshot of the alacritty `TermDamage` state captured by the test-only
/// [`TerminalService::inspect_damage_and_reset`] helper. Used by Phase 1
/// PoC integration tests to characterize the damage API's observable
/// behavior in alacritty 0.26.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageSnapshot {
    /// `Term::damage()` returned `TermDamage::Full` — the entire viewport
    /// is considered dirty.
    Full,
    /// `Term::damage()` returned `TermDamage::Partial(iter)`; `line_count`
    /// is the number of damaged lines yielded by the iterator.
    Partial {
        /// Number of damaged lines yielded by the partial iterator.
        line_count: usize,
    },
}

impl TerminalService {
    /// Register `aid` so the next call to `spawn` with this id
    /// returns `TerminalError::Pty(...)` without touching the real PTY.
    /// Consumed on use.
    pub async fn inject_spawn_failure(&self, aid: ActivityId) {
        self.forced_failures.write().await.insert(aid);
    }

    /// Arm the next call to `spawn` so it returns `TerminalError::Pty(...)`
    /// without touching the real PTY, regardless of the activity id.
    /// Consumed on use. Lets tests exercise spawn-failure paths where the
    /// activity id is generated internally and not known up front.
    pub fn inject_next_spawn_failure(&self) {
        self.next_spawn_fails.store(true, Ordering::SeqCst);
    }

    /// Return the current broadcast subscriber count for an activity, or `None`
    /// if the activity has no PTY. Used in tests to verify task lifecycle.
    pub async fn subscriber_count(&self, activity: &ActivityId) -> Option<usize> {
        let guard = self.read(activity).await.ok()?;
        Some(guard.event_sender.receiver_count())
    }

    /// Test-only read of the VT Term grid: returns the first `cols` characters
    /// of the given `row` as a `String`. Returns `None` if the activity has no
    /// PTY. The VtState lock is short-held and dropped before returning.
    ///
    /// Intended exclusively for integration tests that need to assert the
    /// bridge task has applied PTY output to the in-memory `Term`. Production
    /// code should not depend on this surface.
    pub async fn inspect_row(
        &self,
        activity: &ActivityId,
        row: i32,
        cols: usize,
    ) -> Option<String> {
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let state = vt_state.lock().expect("vt_state lock poisoned");
        let term_row = &state.term.grid()[Line(row)];
        let slice = &term_row[Column(0)..Column(cols)];
        Some(slice.iter().map(|cell| cell.c).collect())
    }

    /// Test-only probe of the alacritty damage tracker: reads
    /// `Term::damage()` into a [`DamageSnapshot`] and then calls
    /// `Term::reset_damage()` under the same VT lock. Returns `None` if
    /// the activity has no PTY.
    ///
    /// Used by Phase 1 PoC tests to characterize the API: whether
    /// `Full` is sticky across resets, how many lines `Partial` reports
    /// after typical shell output, etc.
    pub async fn inspect_damage_and_reset(&self, activity: &ActivityId) -> Option<DamageSnapshot> {
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let mut state = vt_state.lock().expect("vt_state lock poisoned");
        let snapshot = match state.term.damage() {
            TermDamage::Full => DamageSnapshot::Full,
            TermDamage::Partial(iter) => {
                let line_count = iter.count();
                DamageSnapshot::Partial { line_count }
            }
        };
        state.term.reset_damage();
        Some(snapshot)
    }

    /// Returns the current value of `pending_user_input` for the given activity.
    /// Test-only helper that takes the `vt_state` lock briefly.
    pub async fn peek_pending_user_input(&self, activity: &ActivityId) -> TerminalResult<bool> {
        let handle = self.read(activity).await?;
        let state = handle.vt_state.lock().expect("vt_state poisoned");
        Ok(state.pending_user_input)
    }

    /// Returns a clone of the VT chunk sender for the given activity. Test-only.
    /// Allows tests to inject bytes that go straight into the bridge's
    /// `parser.advance`, bypassing the shell so damage timing is deterministic.
    pub async fn vt_chunk_sender_for_test(
        &self,
        activity: &ActivityId,
    ) -> TerminalResult<mpsc::Sender<Bytes>> {
        let handle = self.read(activity).await?;
        Ok(handle.vt_chunk_tx.clone())
    }

    /// Test-only: raw subscription to the wire broadcast (no atomicity guarantee).
    /// Production paths use `subscribe_frames` (Task 13).
    pub async fn subscribe_wire_broadcast(
        &self,
        activity: &ActivityId,
    ) -> Option<broadcast::Receiver<WireMessage>> {
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let state = vt_state.lock().expect("vt_state poisoned");
        Some(state.wire_broadcast.subscribe())
    }

    /// Test-only probe of `TermMode::ALT_SCREEN`. Returns `Some(true)`
    /// when the terminal is currently on the alternate screen buffer,
    /// `Some(false)` when on the primary buffer, and `None` if the
    /// activity has no PTY. Used by Phase 1 PoC tests to verify which
    /// DEC private mode escapes (`?47` / `?1047` / `?1049`) toggle the
    /// alt-screen flag in alacritty 0.26.
    pub async fn inspect_alt_screen(&self, activity: &ActivityId) -> Option<bool> {
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let state = vt_state.lock().expect("vt_state lock poisoned");
        Some(state.term.mode().contains(TermMode::ALT_SCREEN))
    }
}
