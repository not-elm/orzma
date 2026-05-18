//! Unit tests for `PoolHandle` and `BrowserPool` logic.
//!
//! These tests exercise the pool state machine without a live CEF instance.
//! `cef::post_task` requires `CefInitialize`, which cannot be called in a unit
//! test context, so commands are dispatched directly through the lock via
//! `with_pool_mut_for_tests`.

use ozmux_browser_cef_protocol::wire::HostEvent;
use ozmux_cef_host::pool::BrowserPool;
use ozmux_cef_host::post_command::PoolHandle;
use tokio::sync::mpsc;

#[test]
fn pool_handle_shutdown_flag_visible_after_set() {
    // NOTE: BrowserPool::execute(Shutdown) calls cef::quit_message_loop(), which
    // requires a live CefInitialize — not available here. We verify only that the
    // shutdown_requested flag becomes true when set directly; the CEF side is
    // covered by the integration harness.
    // The event_tx receiver is dropped immediately; that's fine for this test
    // since no NavStateChanged events will be emitted.
    let (event_tx, _event_rx) = mpsc::unbounded_channel::<HostEvent>();
    let handle = PoolHandle::new(BrowserPool::new(event_tx, std::env::temp_dir(), false));
    handle.with_pool_mut_for_tests(|p| p.shutdown_requested = true);
    assert!(handle.snapshot_shutdown_requested());
}
