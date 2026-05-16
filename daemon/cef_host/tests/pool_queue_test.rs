//! Unit tests for `PoolHandle` and `BrowserPool` logic.
//!
//! These tests exercise the pool state machine without a live CEF instance.
//! `cef::post_task` requires `CefInitialize`, which cannot be called in a unit
//! test context, so commands are dispatched directly through the lock via
//! `with_pool_mut_for_tests`.

use ozmux_cef_host::pool::{BrowserPool, CefCommand};
use ozmux_cef_host::post_command::PoolHandle;

#[test]
fn pool_handle_shutdown_path_does_not_poison() {
    // NOTE: calls BrowserPool::execute directly through the lock because
    // cef::post_task can only be invoked after CefInitialize, which a unit
    // test cannot do.
    let handle = PoolHandle::new(BrowserPool::new());
    handle.with_pool_mut_for_tests(|p| p.execute(CefCommand::Shutdown));
    assert!(handle.snapshot_shutdown_requested());
}
