//! Integration test (Plan 2 Task A4): spawn the real cef_host binary,
//! complete the daemon-side handshake (Hello/Ready), and verify the
//! supervisor returns valid channels. Per-BrowserCreate shm fds are wired
//! in Task A5 (this test will be extended in Task A14 to assert a real
//! frame is rendered).
//!
//! Gated by `OZMUX_TEST_REAL_CEF=1` because it requires a built cef_host
//! binary and a working CEF framework on disk; CI does not run it.

use ozmux_browser::cef_service::CefHostSupervisor;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn handshake_with_real_cef_host() {
    if std::env::var("OZMUX_TEST_REAL_CEF").ok().as_deref() != Some("1") {
        eprintln!("skipped; set OZMUX_TEST_REAL_CEF=1");
        return;
    }

    let socket = std::path::PathBuf::from("/tmp/ozmux_test_handshake.sock");

    let supervisor = CefHostSupervisor::new(socket);
    let mut handles =
        tokio::time::timeout(Duration::from_secs(10), supervisor.spawn_and_handshake())
            .await
            .expect("handshake timed out")
            .expect("handshake failed");

    // Drop the command sender so the child sees the read side close and
    // exits its select loop.
    drop(handles.commands);

    // Drain any pending events with a short timeout so the test stays bounded
    // when the child does not emit anything before close.
    let _ = tokio::time::timeout(Duration::from_millis(500), handles.events.recv()).await;

    let _ = handles.child.kill().await;
}
