//! Unit tests for the pending counter in `install_cookies`.
//!
//! The non-empty path requires a live `CefCookieManager` and cannot be unit-tested
//! here. It is exercised by the `OZMUX_TEST_REAL_CEF=1` integration test in
//! `daemon/browser/tests/cef_host_handshake.rs`.
//!
//! NOTE: only the empty-list fast path is tested here because `CefCookieManager`
//! is unavailable without a live `CefInitialize`.

use ozmux_cef_host::cookies;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn install_cookies_empty_fires_on_done_synchronously() {
    let fired = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&fired);
    cookies::install_cookies(Vec::new(), move || {
        f.store(true, Ordering::SeqCst);
    });
    assert!(
        fired.load(Ordering::SeqCst),
        "on_done must fire synchronously for the empty-cookie fast path"
    );
}
