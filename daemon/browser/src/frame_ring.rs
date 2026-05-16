//! Per-activity FrameRing for the cef screencast pipeline.
//!
//! PoC scope: only the last keyframe is retained; deltas fan out via
//! `tokio::sync::broadcast` to live subscribers. ResumeReplay (full delta
//! ring backfill) is added in Plan 2.

use bytes::Bytes;
use ozmux_browser_cef_protocol::types::Rect;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// One screencast frame as routed daemon-internally (before websocket framing).
pub struct FrameEnvelope {
    pub session_id: u64,
    pub epoch: u32,
    pub frame_seq: u64,
    pub captured_at_us: u64,
    pub width: u32,
    pub height: u32,
    pub is_keyframe: bool,
    pub damage_rects: Vec<Rect>,
    pub bgra: Bytes,
}

/// Subscription returned to a new WS client: the latest keyframe (if any)
/// plus a live broadcast receiver for subsequent frames. Returning both
/// atomically prevents a frame from slipping through between a `latest`
/// peek and a `subscribe`.
pub struct FrameSubscription {
    pub keyframe: Option<Arc<FrameEnvelope>>,
    pub receiver: broadcast::Receiver<Arc<FrameEnvelope>>,
}

/// FrameRing: PoC stores only `last_keyframe` and a broadcast channel.
pub struct FrameRing {
    inner: Mutex<FrameRingInner>,
    broadcast_tx: broadcast::Sender<Arc<FrameEnvelope>>,
}

struct FrameRingInner {
    epoch: u32,
    last_keyframe: Option<Arc<FrameEnvelope>>,
}

impl FrameRing {
    /// Creates an empty ring for `epoch` with a broadcast capacity of 16.
    pub fn new(epoch: u32) -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            inner: Mutex::new(FrameRingInner {
                epoch,
                last_keyframe: None,
            }),
            broadcast_tx: tx,
        }
    }

    /// The epoch this ring was created at.
    pub fn epoch(&self) -> u32 {
        self.inner.lock().expect("frame ring poisoned").epoch
    }

    /// Pushes a new frame. Keyframes overwrite `last_keyframe`; all frames are
    /// broadcast to live subscribers.
    pub fn push(&self, env: Arc<FrameEnvelope>) {
        {
            let mut inner = self.inner.lock().expect("frame ring poisoned");
            if env.is_keyframe {
                inner.last_keyframe = Some(env.clone());
            }
        }
        // NOTE: send returns Err when there are no live subscribers — that's
        // expected during idle periods, so we discard the result.
        let _ = self.broadcast_tx.send(env);
    }

    /// Atomically captures `(latest_keyframe, receiver)` under the inner lock
    /// so a frame cannot slip in between the keyframe peek and the subscribe.
    pub fn subscribe(&self) -> FrameSubscription {
        let inner = self.inner.lock().expect("frame ring poisoned");
        FrameSubscription {
            keyframe: inner.last_keyframe.clone(),
            receiver: self.broadcast_tx.subscribe(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_env(seq: u64, keyframe: bool) -> Arc<FrameEnvelope> {
        Arc::new(FrameEnvelope {
            session_id: 1,
            epoch: 1,
            frame_seq: seq,
            captured_at_us: seq * 1000,
            width: 1280,
            height: 800,
            is_keyframe: keyframe,
            damage_rects: vec![],
            bgra: Bytes::from_static(&[]),
        })
    }

    #[test]
    fn subscribe_before_keyframe_returns_no_keyframe() {
        let ring = FrameRing::new(1);
        let sub = ring.subscribe();
        assert!(sub.keyframe.is_none());
    }

    #[test]
    fn subscribe_after_keyframe_returns_latest() {
        let ring = FrameRing::new(1);
        ring.push(make_env(1, true));
        ring.push(make_env(2, false));
        ring.push(make_env(3, true));
        let sub = ring.subscribe();
        let kf = sub.keyframe.expect("expected keyframe");
        assert_eq!(kf.frame_seq, 3);
    }

    #[tokio::test]
    async fn pushed_frames_arrive_on_receiver() {
        let ring = FrameRing::new(1);
        ring.push(make_env(1, true));
        let mut sub = ring.subscribe();
        ring.push(make_env(2, false));
        let env = sub.receiver.recv().await.expect("broadcast recv failed");
        assert_eq!(env.frame_seq, 2);
    }
}
