//! Headless browser service for ozmux (cef path).

pub mod cef_backend;
pub mod cef_dispatcher;
pub mod cef_registry;
pub mod cef_service;
pub mod cookie_extractor;
pub mod error;
pub mod frame_ring;
pub mod shm_alloc;
pub mod shm_reader;

pub use cef_service::{CefHostHandles, CefHostSupervisor};
pub use error::{BrowserError, BrowserResult};
pub use frame_ring::{FrameEnvelope, FrameRing, FrameSubscription};
pub use ozmux_browser_cef_protocol::wire::BrowserUnavailableReason;

/// Returns `true` when `OZMUX_TEST_REAL_CHROME=1` is set in the environment.
/// Tests that require a live Chromium process should skip themselves when this
/// returns `false`.
#[cfg(test)]
pub(crate) fn requires_real_chrome() -> bool {
    std::env::var("OZMUX_TEST_REAL_CHROME").ok().as_deref() == Some("1")
}
