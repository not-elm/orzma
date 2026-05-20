//! `ozmux-daemon` binary entry point.
//!
//! Phase 1: thin wrapper that delegates to `daemon_bootstrap::run()` via
//! `#[tokio::main]`. Plan 3 replaces this with a manual runtime + CEF
//! message loop on the main thread.

#[expect(
    deprecated,
    reason = "Plan 3 Task 8 replaces this main.rs with a CEF-aware version using serve(stop_rx)"
)]
fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(daemon_bootstrap::run())
}
