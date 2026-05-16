//! Headless browser service for ozmux. Owns one shared Chromium process,
//! one CDP page per Activity, and a `watch` channel of screencast snapshots
//! per Activity.
//!
//! This crate is in early scaffolding — see
//! `docs/superpowers/plans/2026-05-16-browser-activity.md` Phase 2 for the
//! task breakdown. The public API surface listed below is intentionally
//! sparse for Task 2.1; subsequent tasks fill it in.

pub(crate) mod bridge;
pub(crate) mod bytes_serde;
pub mod cookie;
pub mod error;
pub mod input;
pub mod page;
pub mod service;
pub mod snapshot;
pub mod state;
pub mod wire;

pub use error::{BrowserError, BrowserResult};
pub use service::BrowserService;
pub use snapshot::{BrowserSnapshot, NavState, ScreencastFrame};
pub use wire::{BrowserClientMsg, BrowserServerMsg, KeyKind, MouseButton, MouseKind, NavCommand};

/// Returns `true` when `OZMUX_TEST_REAL_CHROME=1` is set in the environment.
/// Tests that require a live Chromium process should skip themselves when this
/// returns `false`.
#[cfg(test)]
pub(crate) fn requires_real_chrome() -> bool {
    std::env::var("OZMUX_TEST_REAL_CHROME").ok().as_deref() == Some("1")
}
