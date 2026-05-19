//! Test/dev-only infrastructure for ozmux_terminal: the PTY tape replay
//! harness, criterion bench helpers, and the `?replay=` debug-only route's
//! backend.
//!
//! Gated by the `test-helpers` Cargo feature in lib.rs so production release
//! builds never compile any of this code. See Section 3 "Module placement &
//! gating" in `docs/superpowers/specs/2026-05-19-pr-a-replay-harness-design.md`.
pub mod replay;
pub mod tape;

mod player;

#[cfg(test)]
mod tests;
