//! Unit tests for `PoolHandle` and `BrowserPool` logic.
//!
//! These tests exercise the pool state machine without a live CEF instance.
//! `cef::post_task` requires `CefInitialize`, which cannot be called in a unit
//! test context, so commands are dispatched directly through the lock via
//! `with_pool_mut_for_tests`.

use ozmux_cef_host::pool::BrowserPool;
use ozmux_cef_host::post_command::PoolHandle;

#[test]
fn pool_handle_shutdown_flag_visible_after_set() {
    // NOTE: BrowserPool::execute(Shutdown) calls cef::quit_message_loop(), which
    // requires a live CefInitialize — not available here. We verify only that the
    // shutdown_requested flag becomes true when set directly; the CEF side is
    // covered by the integration harness.
    let handle = PoolHandle::new(BrowserPool::new());
    handle.with_pool_mut_for_tests(|p| p.shutdown_requested = true);
    assert!(handle.snapshot_shutdown_requested());
}
