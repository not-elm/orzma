//! Bounded scrollback ring shared between the PTY reader and WS subscribers.

use crate::event::TerminalEvent;
use crate::pty::ring_buffer::RingBuffer;
use std::{num::NonZero, sync::Arc};
use tokio::sync::{
    Mutex,
    broadcast::{Receiver, Sender},
};

/// Fixed-capacity raw-byte scrollback shared by the PTY reader (producer)
/// and WebSocket subscribers (consumer).
#[derive(Clone, Debug)]
pub(crate) struct ScrollbackBuffer(Arc<Mutex<RingBuffer>>);

impl ScrollbackBuffer {
    const SCROLLBACK_BYTES: usize = 256 * 1024;

    /// Allocates a new ring with the crate-wide scrollback capacity.
    pub fn new() -> Self {
        let capacity = NonZero::new(Self::SCROLLBACK_BYTES).unwrap();
        Self(Arc::new(Mutex::new(RingBuffer::with_capacity(capacity))))
    }

    /// Bare push/snapshot are test-only — production code MUST use
    /// `push_and_broadcast` and `snapshot_and_subscribe` to keep the producer
    /// and consumer sides serialized through a single critical section.
    #[cfg(test)]
    #[expect(
        dead_code,
        reason = "test-only helper; production goes through push_and_broadcast"
    )]
    #[inline]
    pub async fn push(&self, data: &[u8]) {
        self.0.lock().await.push(data);
    }

    /// Returns a snapshot of the current scrollback contents (test-only).
    #[cfg(test)]
    #[inline]
    pub async fn snapshot(&self) -> Vec<u8> {
        self.0.lock().await.snapshot()
    }

    /// Producer-side primitive: push a chunk and broadcast under the same lock.
    /// Used by the PTY bridge task. Races with `snapshot_and_subscribe` are
    /// serialized through the scrollback mutex.
    pub async fn push_and_broadcast(&self, sender: &Sender<TerminalEvent>, chunk: Vec<u8>) {
        let mut guard = self.0.lock().await;
        guard.push(&chunk);
        let _ = sender.send(TerminalEvent::Data { buffer: chunk });
    }

    /// Consumer-side primitive: take the scrollback snapshot AND subscribe to
    /// the broadcast channel under the same lock. Used by the WS handler at
    /// connect time. Guarantees zero duplicates and zero gaps.
    pub async fn snapshot_and_subscribe(
        &self,
        sender: &Sender<TerminalEvent>,
    ) -> (Vec<u8>, Receiver<TerminalEvent>) {
        let guard = self.0.lock().await;
        let snap = guard.snapshot();
        let rx = sender.subscribe();
        (snap, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TerminalEvent;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn push_and_broadcast_writes_to_scrollback_and_sends_event() {
        let scrollback = ScrollbackBuffer::new();
        let (tx, mut rx) = broadcast::channel(16);

        scrollback.push_and_broadcast(&tx, b"hello".to_vec()).await;

        assert_eq!(scrollback.snapshot().await, b"hello");
        match rx.recv().await.unwrap() {
            TerminalEvent::Data { buffer } => assert_eq!(buffer, b"hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn snapshot_and_subscribe_captures_prior_pushes_only_in_snapshot() {
        let scrollback = ScrollbackBuffer::new();
        let (tx, _) = broadcast::channel(16);

        scrollback.push_and_broadcast(&tx, b"old".to_vec()).await;

        let (snap, mut rx) = scrollback.snapshot_and_subscribe(&tx).await;
        assert_eq!(snap, b"old");

        scrollback.push_and_broadcast(&tx, b"new".to_vec()).await;

        match rx.recv().await.unwrap() {
            TerminalEvent::Data { buffer } => assert_eq!(buffer, b"new"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(rx.try_recv().is_err());
    }
}
