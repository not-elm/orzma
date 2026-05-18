//! Per-activity FrameRing for the cef screencast pipeline.
//!
//! Plan 2 Task A7: retains the latest keyframe plus every subsequent delta
//! up to `RING_BYTES_BUDGET` / `RING_FRAMES_BUDGET` (parent §20.9). Subscribers
//! receive (keyframe, replay_deltas, broadcast receiver) atomically; stale
//! `last_key`s produce `MustRestart`.

use bytes::Bytes;
use ozmux_browser_cef_protocol::types::{FrameKey, Rect};
pub use ozmux_browser_cef_protocol::wire::MustRestartReason;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Memory budget for retained deltas (parent §20.9).
pub(crate) const RING_BYTES_BUDGET: usize = 128 * 1024 * 1024;
/// Frame-count budget for retained deltas (parent §20.9).
pub(crate) const RING_FRAMES_BUDGET: usize = 60;
const BROADCAST_CAPACITY: usize = 16;

/// One screencast frame as routed daemon-internally (before websocket framing).
#[derive(Debug, Clone)]
pub struct FrameEnvelope {
    /// Daemon-wide session identifier, stamped from `BrowserCefRegistry::session_id`.
    pub session_id: u64,
    /// Monotonic epoch counter; increments on cef_host respawn.
    pub epoch: u32,
    /// Monotonic frame sequence counter within an epoch.
    pub frame_seq: u64,
    /// Wall-clock capture timestamp in microseconds.
    pub captured_at_us: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// `true` for keyframes; `false` for delta frames.
    pub is_keyframe: bool,
    /// Damaged regions within the frame.
    pub damage_rects: Vec<Rect>,
    /// `true` when this frame belongs to a popup overlay.
    pub is_popup: bool,
    /// Raw BGRA pixel data.
    pub bgra: Bytes,
}

impl FrameEnvelope {
    /// Returns the `(session_id, epoch, frame_seq)` triple that uniquely
    /// identifies this frame in a ResumeReplay context.
    pub fn key(&self) -> FrameKey {
        FrameKey {
            session_id: self.session_id,
            epoch: self.epoch,
            frame_seq: self.frame_seq,
        }
    }
}

/// Inputs to `FrameRing::subscribe`.
pub struct SubscribeRequest {
    /// Session id from the client handshake, or `0` to skip session checking.
    pub session_id: u64,
    /// The last frame key the client already has, for resume/replay detection.
    pub last_key: Option<FrameKey>,
    /// Whether the client already holds a base keyframe for the current session.
    pub has_base_keyframe: bool,
}

/// Output of `FrameRing::subscribe`.
pub enum FrameSubscription {
    /// Client has no prior state: send the current keyframe + deltas, then stream.
    FreshSnapshot {
        /// The most recent keyframe.
        keyframe: Arc<FrameEnvelope>,
        /// All deltas since the keyframe.
        deltas: Vec<Arc<FrameEnvelope>>,
        /// Live broadcast receiver for subsequent frames.
        receiver: broadcast::Receiver<Arc<FrameEnvelope>>,
    },
    /// Client has a recent keyframe: send only the missing deltas, then stream.
    ResumeReplay {
        /// Deltas the client is missing since its `last_key`.
        deltas: Vec<Arc<FrameEnvelope>>,
        /// Live broadcast receiver for subsequent frames.
        receiver: broadcast::Receiver<Arc<FrameEnvelope>>,
    },
    /// The client's prior state is incompatible; it must re-subscribe from scratch.
    MustRestart {
        /// Reason for the restart requirement.
        reason: MustRestartReason,
    },
    /// No keyframe has been received yet; client should wait for the broadcast.
    AwaitingKeyframe {
        /// Live broadcast receiver; the first keyframe will arrive here.
        receiver: broadcast::Receiver<Arc<FrameEnvelope>>,
    },
}

/// Per-activity frame ring. PoC of Plan 1 kept only the latest keyframe; Plan 2
/// retains the latest keyframe plus every delta since, bounded by the budgets
/// above. Subscribers receive an atomic snapshot via `subscribe()`.
pub struct FrameRing {
    inner: Mutex<FrameRingInner>,
    broadcast_tx: broadcast::Sender<Arc<FrameEnvelope>>,
    session_id: u64,
}

struct FrameRingInner {
    epoch: u32,
    last_keyframe: Option<Arc<FrameEnvelope>>,
    deltas: VecDeque<Arc<FrameEnvelope>>,
    deltas_bytes: usize,
}

impl FrameRing {
    /// Builds an empty ring stamped with `session_id` (daemon-wide) and `epoch`
    /// (per cef_host respawn; Plan 2 keeps it fixed at 1).
    pub fn new(session_id: u64, epoch: u32) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Mutex::new(FrameRingInner {
                epoch,
                last_keyframe: None,
                deltas: VecDeque::with_capacity(RING_FRAMES_BUDGET),
                deltas_bytes: 0,
            }),
            broadcast_tx: tx,
            session_id,
        }
    }

    /// Current epoch.
    pub fn epoch(&self) -> u32 {
        self.inner.lock().expect("frame ring poisoned").epoch
    }

    /// Session id assigned at construction.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Appends `env` to the ring. Keyframes reset the delta backlog; deltas
    /// extend it (evicting oldest entries when the budgets are exceeded). The
    /// envelope is then broadcast to every live subscriber.
    pub fn push(&self, env: Arc<FrameEnvelope>) {
        {
            let mut inner = self.inner.lock().expect("frame ring poisoned");
            if env.is_keyframe {
                inner.last_keyframe = Some(env.clone());
                inner.deltas.clear();
                inner.deltas_bytes = 0;
            } else if inner.last_keyframe.is_some() {
                inner.deltas.push_back(env.clone());
                inner.deltas_bytes += env.bgra.len();
                while inner.deltas.len() > RING_FRAMES_BUDGET
                    || inner.deltas_bytes > RING_BYTES_BUDGET
                {
                    if let Some(dropped) = inner.deltas.pop_front() {
                        inner.deltas_bytes = inner.deltas_bytes.saturating_sub(dropped.bgra.len());
                    } else {
                        break;
                    }
                }
            }
            // NOTE: deltas pushed before the first keyframe are dropped on the
            // floor — the subscriber sees `AwaitingKeyframe` instead.
        }
        // NOTE: broadcast::send returns Err when there are no live
        // subscribers — that is the steady-state idle case, not a failure.
        let _ = self.broadcast_tx.send(env);
    }

    /// Atomically captures `(latest_keyframe, deltas)` under the inner lock and
    /// subscribes to the live broadcast so a frame cannot slip in between the
    /// peek and the subscription.
    pub fn subscribe(&self, req: SubscribeRequest) -> FrameSubscription {
        let inner = self.inner.lock().expect("frame ring poisoned");
        let rx = self.broadcast_tx.subscribe();

        if req.session_id != 0 && req.session_id != self.session_id {
            return FrameSubscription::MustRestart {
                reason: MustRestartReason::SessionMismatch,
            };
        }
        let Some(keyframe) = inner.last_keyframe.clone() else {
            return FrameSubscription::AwaitingKeyframe { receiver: rx };
        };
        if let Some(last) = req.last_key {
            if last.epoch != inner.epoch {
                return FrameSubscription::MustRestart {
                    reason: MustRestartReason::EpochMismatch,
                };
            }
            // last_key matches a known delta — replay everything after it.
            if let Some(idx) = inner.deltas.iter().position(|d| d.key() == last) {
                let backfill: Vec<_> = inner.deltas.iter().skip(idx + 1).cloned().collect();
                return FrameSubscription::ResumeReplay {
                    deltas: backfill,
                    receiver: rx,
                };
            }
            // last_key matches the current keyframe directly — replay every delta.
            if keyframe.key() == last {
                let backfill: Vec<_> = inner.deltas.iter().cloned().collect();
                return FrameSubscription::ResumeReplay {
                    deltas: backfill,
                    receiver: rx,
                };
            }
            return FrameSubscription::MustRestart {
                reason: MustRestartReason::LastKeyEvicted,
            };
        }
        if req.has_base_keyframe {
            let backfill: Vec<_> = inner.deltas.iter().cloned().collect();
            return FrameSubscription::ResumeReplay {
                deltas: backfill,
                receiver: rx,
            };
        }
        let snapshot_deltas: Vec<_> = inner.deltas.iter().cloned().collect();
        FrameSubscription::FreshSnapshot {
            keyframe,
            deltas: snapshot_deltas,
            receiver: rx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(seq: u64, keyframe: bool, sz: usize) -> Arc<FrameEnvelope> {
        Arc::new(FrameEnvelope {
            session_id: 7,
            epoch: 1,
            frame_seq: seq,
            captured_at_us: 0,
            width: 100,
            height: 50,
            is_keyframe: keyframe,
            damage_rects: vec![],
            is_popup: false,
            bgra: bytes::Bytes::from(vec![0u8; sz]),
        })
    }

    #[test]
    fn fresh_snapshot_after_keyframe() {
        let r = FrameRing::new(7, 1);
        r.push(env(1, true, 100));
        r.push(env(2, false, 50));
        match r.subscribe(SubscribeRequest {
            session_id: 7,
            last_key: None,
            has_base_keyframe: false,
        }) {
            FrameSubscription::FreshSnapshot {
                keyframe, deltas, ..
            } => {
                assert_eq!(keyframe.frame_seq, 1);
                assert_eq!(deltas.len(), 1);
                assert_eq!(deltas[0].frame_seq, 2);
            }
            _ => panic!("expected FreshSnapshot"),
        }
    }

    #[test]
    fn resume_replay_after_known_last_key() {
        let r = FrameRing::new(7, 1);
        r.push(env(1, true, 100));
        r.push(env(2, false, 50));
        r.push(env(3, false, 50));
        let last = FrameKey {
            session_id: 7,
            epoch: 1,
            frame_seq: 2,
        };
        match r.subscribe(SubscribeRequest {
            session_id: 7,
            last_key: Some(last),
            has_base_keyframe: true,
        }) {
            FrameSubscription::ResumeReplay { deltas, .. } => {
                assert_eq!(deltas.len(), 1);
                assert_eq!(deltas[0].frame_seq, 3);
            }
            _ => panic!("expected ResumeReplay"),
        }
    }

    #[test]
    fn must_restart_on_session_mismatch() {
        let r = FrameRing::new(7, 1);
        r.push(env(1, true, 100));
        match r.subscribe(SubscribeRequest {
            session_id: 999,
            last_key: None,
            has_base_keyframe: false,
        }) {
            FrameSubscription::MustRestart {
                reason: MustRestartReason::SessionMismatch,
            } => {}
            _ => panic!("expected SessionMismatch"),
        }
    }

    #[test]
    fn must_restart_when_last_key_evicted() {
        // Push a keyframe + (RING_FRAMES_BUDGET + 9) deltas so the earliest
        // deltas (frame_seq 2..=10) are evicted by the frame-count budget.
        // Request resume from a frame that was once present but has been
        // evicted to confirm the LastKeyEvicted path.
        let r = FrameRing::new(7, 1);
        r.push(env(1, true, 100));
        for i in 2..=(RING_FRAMES_BUDGET as u64 + 10) {
            r.push(env(i, false, 1));
        }
        let last = FrameKey {
            session_id: 7,
            epoch: 1,
            frame_seq: 2,
        };
        match r.subscribe(SubscribeRequest {
            session_id: 7,
            last_key: Some(last),
            has_base_keyframe: true,
        }) {
            FrameSubscription::MustRestart {
                reason: MustRestartReason::LastKeyEvicted,
            } => {}
            _ => panic!("expected LastKeyEvicted"),
        }
    }

    #[test]
    fn budget_evicts_oldest() {
        let r = FrameRing::new(7, 1);
        r.push(env(1, true, 0));
        for i in 2..=(2 + RING_FRAMES_BUDGET as u64) {
            r.push(env(i, false, 0));
        }
        let inner = r.inner.lock().unwrap();
        assert!(inner.deltas.len() <= RING_FRAMES_BUDGET);
    }
}
