//! Integration tests for per-aid `BrowserUnavailable` signaling on the
//! `LiveCefDispatcher` broadcast channel.

use ozmux_browser::cef_dispatcher::CefDispatcher;
use ozmux_browser::cef_dispatcher::live::LiveCefDispatcher;
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableReason, HostEvent};
use ozmux_cef_host::FrameBufferPool;
use ozmux_cef_host::pool::BrowserPool;
use ozmux_cef_host::post_command::PoolHandle;
use ozmux_extension::registry::ExtensionRegistry;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn make_dispatcher() -> LiveCefDispatcher {
    let (event_tx, _event_rx_pool) = mpsc::unbounded_channel::<HostEvent>();
    let (_bnd_tx, bnd_rx) = mpsc::channel::<HostEvent>(8);
    let frame_pool = Arc::new(FrameBufferPool::new(4));
    let extensions = ExtensionRegistry::default();
    let handle = PoolHandle::new(BrowserPool::new(
        event_tx,
        std::env::temp_dir(),
        false,
        0,
        frame_pool,
        extensions,
    ));
    LiveCefDispatcher::new(handle, bnd_rx)
}

#[tokio::test(flavor = "current_thread")]
async fn mark_unavailable_per_aid_observable_to_subscriber() {
    let d = make_dispatcher();
    let mut rx = d.unavailable_subscribe();

    let aid = ActivityId("a-ext-7".into());
    d.mark_unavailable(
        Some(aid.clone()),
        BrowserUnavailableReason::ExtensionDisconnected,
    );

    let ev = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("did not receive within 500ms")
        .expect("channel closed");

    assert_eq!(ev.aid.as_ref(), Some(&aid));
    assert!(matches!(
        ev.reason,
        BrowserUnavailableReason::ExtensionDisconnected
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn mark_unavailable_daemon_wide_uses_none_aid() {
    let d = make_dispatcher();
    let mut rx = d.unavailable_subscribe();

    d.mark_unavailable(
        None,
        BrowserUnavailableReason::RetryExhausted {
            last_error: "boom".into(),
        },
    );

    let ev = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("did not receive within 500ms")
        .expect("channel closed");

    assert!(ev.aid.is_none());
    assert!(matches!(
        ev.reason,
        BrowserUnavailableReason::RetryExhausted { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn mark_unavailable_with_no_subscribers_does_not_panic() {
    let d = make_dispatcher();
    // No subscribe(); sender::send returns Err which mark_unavailable
    // swallows. The test asserts the call simply returns.
    d.mark_unavailable(
        Some(ActivityId("orphan".into())),
        BrowserUnavailableReason::ExtensionDisconnected,
    );
}
