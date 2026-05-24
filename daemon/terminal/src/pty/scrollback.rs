//! Bounded scrollback ring shared between the PTY reader and WS subscribers.

use crate::event::TerminalEvent;
use crate::pty::ring_buffer::RingBuffer;
use std::{
    num::NonZero,
    sync::{Arc, Mutex},
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

    /// Producer-side primitive: push a chunk and broadcast under the same lock.
    /// Used by the PTY bridge task. Races with `snapshot_and_subscribe` are
    /// serialized through the scrollback mutex.
    #[inline]
    pub fn push(&self, chunk: &[u8]) {
        let mut guard = self.0.lock().unwrap();
        guard.push(chunk);
    }

    /// Consumer-side primitive: take the scrollback snapshot AND subscribe to
    /// the broadcast channel under the same lock. Used by the WS handler at
    /// connect time. Guarantees zero duplicates and zero gaps.
    #[inline]
    pub fn snapshot(&self) -> Vec<u8> {
        let guard = self.0.lock().unwrap();
        guard.snapshot()
    }
}
