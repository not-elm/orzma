use ozmux_multiplexer::window::WindowId;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Per-window WS broadcaster. Each window has at most one `broadcast::Sender`,
/// created on first subscribe. Senders carry full `WindowView` JSON snapshots.
#[derive(Clone)]
pub struct LayoutBroadcaster {
    inner: std::sync::Arc<Mutex<HashMap<WindowId, broadcast::Sender<Value>>>>,
    capacity: usize,
}

impl Default for LayoutBroadcaster {
    fn default() -> Self {
        Self::new(32)
    }
}

impl LayoutBroadcaster {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(HashMap::new())),
            capacity,
        }
    }

    pub fn from_env() -> Self {
        let capacity = std::env::var("OZMUX_LAYOUT_BROADCAST_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32usize);
        Self::new(capacity)
    }

    /// Look up an existing sender or create one. Returns a fresh receiver.
    pub fn subscribe_or_create(&self, wid: &WindowId) -> broadcast::Receiver<Value> {
        let mut map = self
            .inner
            .lock()
            .expect("layout broadcaster mutex poisoned");
        let sender = map
            .entry(wid.clone())
            .or_insert_with(|| broadcast::channel(self.capacity).0);
        sender.subscribe()
    }

    /// Best-effort publish. If no sender exists (no subscribers yet) the call is a no-op.
    /// Errors from `Sender::send` (no receivers) are intentionally swallowed.
    pub fn publish(&self, wid: &WindowId, view: Value) {
        let map = self
            .inner
            .lock()
            .expect("layout broadcaster mutex poisoned");
        if let Some(sender) = map.get(wid) {
            let _ = sender.send(view);
        }
    }

    /// Drop the sender for `wid`, causing existing receivers to observe `RecvError::Closed`.
    pub fn close(&self, wid: &WindowId) {
        let mut map = self
            .inner
            .lock()
            .expect("layout broadcaster mutex poisoned");
        map.remove(wid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::broadcast::error::RecvError;

    fn fresh_wid() -> WindowId {
        WindowId::new()
    }

    #[tokio::test]
    async fn subscribe_or_create_returns_receiver_and_publish_delivers() {
        let bc = LayoutBroadcaster::new(8);
        let wid = fresh_wid();
        let mut rx = bc.subscribe_or_create(&wid);
        bc.publish(&wid, json!({ "id": wid.as_ref() }));
        let v = rx.recv().await.unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
    }

    #[tokio::test]
    async fn publish_to_unknown_window_is_noop() {
        let bc = LayoutBroadcaster::new(8);
        bc.publish(&fresh_wid(), json!({}));
        // No panic, no error — silently dropped.
    }

    #[tokio::test]
    async fn close_kicks_subscribers_with_recv_error_closed() {
        let bc = LayoutBroadcaster::new(8);
        let wid = fresh_wid();
        let mut rx = bc.subscribe_or_create(&wid);
        bc.close(&wid);
        let err = rx.recv().await.expect_err("expected closed");
        assert!(matches!(err, RecvError::Closed));
    }

    #[tokio::test]
    async fn lagged_when_capacity_exceeded() {
        let bc = LayoutBroadcaster::new(2);
        let wid = fresh_wid();
        let mut rx = bc.subscribe_or_create(&wid);
        bc.publish(&wid, json!({ "n": 1 }));
        bc.publish(&wid, json!({ "n": 2 }));
        bc.publish(&wid, json!({ "n": 3 }));
        match rx.recv().await {
            Err(RecvError::Lagged(_)) => {}
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let bc = LayoutBroadcaster::new(8);
        let wid = fresh_wid();
        let mut rx_a = bc.subscribe_or_create(&wid);
        let mut rx_b = bc.subscribe_or_create(&wid);
        bc.publish(&wid, json!({ "n": 7 }));
        let a = rx_a.recv().await.unwrap();
        let b = rx_b.recv().await.unwrap();
        assert_eq!(a["n"].as_u64(), Some(7));
        assert_eq!(b["n"].as_u64(), Some(7));
    }

    #[tokio::test]
    async fn from_env_uses_default_when_unset() {
        // SAFETY: tests in this crate run with `--test-threads=1` is NOT
        // guaranteed; `remove_var` is a process-global mutation. This test is
        // best-effort and may be racy with other env-touching tests run in
        // parallel. Acceptable because no other tests touch this var.
        unsafe {
            std::env::remove_var("OZMUX_LAYOUT_BROADCAST_CAPACITY");
        }
        let bc = LayoutBroadcaster::from_env();
        assert_eq!(bc.capacity, 32);
    }
}
