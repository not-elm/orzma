//! Cached check for the `OZMUX_PERF_PRODUCED_AT` env var so the bridge hot
//! path does not pay a `getenv` syscall per frame.

use std::sync::OnceLock;

static PRODUCED_AT_ENABLED: OnceLock<bool> = OnceLock::new();

/// Returns true if `OZMUX_PERF_PRODUCED_AT=1` was set at process start.
/// The decision is captured the first time the function is called.
pub fn produced_at_enabled() -> bool {
    *PRODUCED_AT_ENABLED
        .get_or_init(|| matches!(std::env::var("OZMUX_PERF_PRODUCED_AT").as_deref(), Ok("1")))
}
