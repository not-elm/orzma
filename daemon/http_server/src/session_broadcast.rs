//! Per-session WS broadcaster. Each session has at most one
//! `broadcast::Sender`, created on first subscribe. Senders carry
//! `serde_json::Value` snapshots of the session view; the concrete
//! `SessionView` shape lives elsewhere.

use ozmux_multiplexer::SessionId;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Per-session WS broadcaster.
#[derive(Clone)]
pub struct SessionBroadcaster {
    inner: std::sync::Arc<Mutex<HashMap<SessionId, broadcast::Sender<Value>>>>,
    capacity: usize,
}

impl Default for SessionBroadcaster {
    fn default() -> Self {
        Self::new(32)
    }
}

impl SessionBroadcaster {
    /// Build a broadcaster with a fixed per-channel capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(HashMap::new())),
            capacity,
        }
    }

    /// Build a broadcaster whose capacity is read from
    /// `OZMUX_SESSION_BROADCAST_CAPACITY`, defaulting to 32.
    pub fn from_env() -> Self {
        let capacity = std::env::var("OZMUX_SESSION_BROADCAST_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32usize);
        Self::new(capacity)
    }

    /// Look up an existing sender or create one. Returns a fresh receiver.
    pub fn subscribe_or_create(&self, sid: &SessionId) -> broadcast::Receiver<Value> {
        let mut map = self
            .inner
            .lock()
            .expect("session broadcaster mutex poisoned");
        let sender = map
            .entry(sid.clone())
            .or_insert_with(|| broadcast::channel(self.capacity).0);
        sender.subscribe()
    }

    /// Best-effort publish. No-op if no sender exists or there are no receivers.
    pub fn publish(&self, sid: &SessionId, view: Value) {
        let map = self
            .inner
            .lock()
            .expect("session broadcaster mutex poisoned");
        if let Some(sender) = map.get(sid) {
            let _ = sender.send(view);
        }
    }

    /// Drop the sender for `sid`, causing existing receivers to observe
    /// `RecvError::Closed`.
    pub fn close(&self, sid: &SessionId) {
        let mut map = self
            .inner
            .lock()
            .expect("session broadcaster mutex poisoned");
        map.remove(sid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::broadcast::error::RecvError;

    fn fresh_sid() -> SessionId {
        SessionId::new()
    }

    #[tokio::test]
    async fn subscribe_or_create_returns_receiver_and_publish_delivers() {
        let bc = SessionBroadcaster::new(8);
        let sid = fresh_sid();
        let mut rx = bc.subscribe_or_create(&sid);
        bc.publish(&sid, json!({ "id": sid.as_ref() }));
        let v = rx.recv().await.unwrap();
        assert_eq!(v["id"].as_str(), Some(sid.as_ref()));
    }

    #[tokio::test]
    async fn publish_to_unknown_session_is_noop() {
        let bc = SessionBroadcaster::new(8);
        bc.publish(&fresh_sid(), json!({}));
    }

    #[tokio::test]
    async fn close_kicks_subscribers_with_recv_error_closed() {
        let bc = SessionBroadcaster::new(8);
        let sid = fresh_sid();
        let mut rx = bc.subscribe_or_create(&sid);
        bc.close(&sid);
        let err = rx.recv().await.expect_err("expected closed");
        assert!(matches!(err, RecvError::Closed));
    }

    #[tokio::test]
    async fn lagged_when_capacity_exceeded() {
        let bc = SessionBroadcaster::new(2);
        let sid = fresh_sid();
        let mut rx = bc.subscribe_or_create(&sid);
        bc.publish(&sid, json!({ "n": 1 }));
        bc.publish(&sid, json!({ "n": 2 }));
        bc.publish(&sid, json!({ "n": 3 }));
        match rx.recv().await {
            Err(RecvError::Lagged(_)) => {}
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let bc = SessionBroadcaster::new(8);
        let sid = fresh_sid();
        let mut rx_a = bc.subscribe_or_create(&sid);
        let mut rx_b = bc.subscribe_or_create(&sid);
        bc.publish(&sid, json!({ "n": 7 }));
        let a = rx_a.recv().await.unwrap();
        let b = rx_b.recv().await.unwrap();
        assert_eq!(a["n"].as_u64(), Some(7));
        assert_eq!(b["n"].as_u64(), Some(7));
    }

    #[tokio::test]
    async fn from_env_uses_default_when_unset() {
        // SAFETY: std::env::remove_var is `unsafe` because concurrent
        // env access from any thread in the process is UB. This test
        // is the sole reader/writer of OZMUX_SESSION_BROADCAST_CAPACITY
        // in this test binary, and tokio's `#[tokio::test]` does not
        // spawn additional env-reading threads, so the call is sound.
        unsafe {
            std::env::remove_var("OZMUX_SESSION_BROADCAST_CAPACITY");
        }
        let bc = SessionBroadcaster::from_env();
        assert_eq!(bc.capacity, 32);
    }
}
