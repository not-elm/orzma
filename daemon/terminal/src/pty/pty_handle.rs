use crate::pty::{TerminalEvent, ring_buffer::RingBuffer};
use crate::vt::bridge::{VtState, run_bridge_task};
use crate::vt::frame_ring::EncodedDelta;
use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
use bytes::Bytes;
use portable_pty::{ChildKiller, MasterPty};
use std::sync::atomic::AtomicU32;
use std::{io::Write, num::NonZero, sync::Arc};
use tokio::sync::{
    Mutex, broadcast,
    broadcast::{Receiver, Sender},
    mpsc,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(crate) struct ScrollbackBuffer(Arc<Mutex<RingBuffer>>);

impl ScrollbackBuffer {
    const SCROLLBACK_BYTES: usize = 256 * 1024;
    pub fn new() -> Self {
        let capacity = NonZero::new(Self::SCROLLBACK_BYTES).unwrap();
        Self(Arc::new(Mutex::new(RingBuffer::with_capacity(capacity))))
    }

    /// Bare push/snapshot are test-only — production code MUST use
    /// `push_and_broadcast` and `snapshot_and_subscribe` to keep the producer
    /// and consumer sides serialized through a single critical section.
    #[cfg(test)]
    #[allow(dead_code)]
    #[inline]
    pub async fn push(&self, data: &[u8]) {
        self.0.lock().await.push(data);
    }

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

pub(crate) struct PtyHandle {
    pub master: Mutex<Box<dyn MasterPty + Send>>,
    pub writer: Mutex<Box<dyn Write + Send>>,
    pub scrollback: ScrollbackBuffer,
    pub event_sender: Sender<TerminalEvent>,
    _child_killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,

    // === VT path (Phase 1+) ===
    /// Bundled VT state (Term + Parser + FrameRing + last_input_at).
    /// Wrapped in std::sync::Mutex for short-held locks per PTY chunk
    /// in vt_bridge_task (Task 13).
    #[expect(dead_code, reason = "wired in Task 13/14")]
    pub(crate) vt_state: Arc<std::sync::Mutex<VtState>>,

    /// reply-required events from TermListener (unbounded; must-not-drop
    /// to avoid capability-query backflow regression).
    #[expect(dead_code, reason = "wired in Task 13/14")]
    pub(crate) reply_tx: mpsc::UnboundedSender<ReplyFrame>,

    /// best-effort control events (bounded cap=64); try_send drops are
    /// rate-limited via DropCounter.
    #[expect(dead_code, reason = "wired in Task 13/14")]
    pub(crate) control_tx: mpsc::Sender<ControlFrame>,

    /// broadcast of MessagePack-encoded delta frames. Phase 1: no
    /// emissions; Phase 2 wires the bridge task to push here.
    #[expect(dead_code, reason = "wired in Task 13/14")]
    pub(crate) frame_broadcast: broadcast::Sender<EncodedDelta>,

    /// Monotonic frame sequence number across reconnects.
    /// Activity-scoped, resets when the daemon process restarts.
    #[expect(dead_code, reason = "wired in Task 13/14")]
    pub(crate) frame_seq: AtomicU32,

    /// Aggregated drop counter for bounded channel try_send failures.
    #[expect(dead_code, reason = "wired in Task 13/14")]
    pub(crate) drop_counter: Arc<DropCounter>,

    /// Sender used by `spawn_pty_reader`'s bridge task to fan-out PTY chunks
    /// to the VT bridge. Bounded cap=128 — overflow is dropped silently
    /// (the raw scrollback path is the source of truth). The actual fan-out
    /// sender lives in the spawned bridge task; this handle-side copy keeps
    /// the channel open for the lifetime of `PtyHandle` and is available
    /// for future direct injection (e.g., synthetic VT input in tests).
    #[expect(dead_code, reason = "channel keepalive; consumer added in Phase 2")]
    pub(crate) vt_chunk_tx: mpsc::Sender<Bytes>,

    /// Cancellation for the VT bridge task; cancelled on `PtyHandle::drop`.
    pub(crate) vt_cancel: CancellationToken,
}

impl PtyHandle {
    #[allow(
        clippy::too_many_arguments,
        reason = "constructor wires raw PTY + VT bridge resources; a builder \
                  would obscure the single call site in TerminalService::spawn"
    )]
    pub fn new(
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        event_sender: Sender<TerminalEvent>,
        child_killer: Box<dyn ChildKiller + Send + Sync>,
        scrollback: ScrollbackBuffer,
        cols: u16,
        rows: u16,
        vt_chunk_tx: mpsc::Sender<Bytes>,
        vt_chunk_rx: mpsc::Receiver<Bytes>,
    ) -> Self {
        // ===== VT path setup (Phase 1) =====
        // Phase 1: keep `reply_rx` / `control_rx` and pass them to the bridge
        // task. The `frame_rx` broadcast receiver is dropped — Phase 2's
        // `subscribe_frames` API will create fresh receivers via
        // `frame_broadcast.subscribe()`.
        let (reply_tx, reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, control_rx) = mpsc::channel::<ControlFrame>(64);
        let (frame_broadcast, _frame_rx) = broadcast::channel::<EncodedDelta>(256);
        let drop_counter = Arc::new(DropCounter::new());

        let listener = TermListener {
            reply_tx: reply_tx.clone(),
            control_tx: control_tx.clone(),
            drop_counter: drop_counter.clone(),
        };

        let vt_state = Arc::new(std::sync::Mutex::new(VtState::new(cols, rows, listener)));

        // VT chunk channel is created by the caller and split so the raw
        // bridge task can hold the sender (fan-out) while we pass the receiver
        // to `run_bridge_task`. Cancellation lives here.
        let vt_cancel = CancellationToken::new();

        // Phase 1: the JoinHandle is discarded; Task 15 may want to await it
        // on shutdown. Cancellation via `vt_cancel` on drop is sufficient for
        // task termination.
        let _vt_join = tokio::spawn(run_bridge_task(
            vt_state.clone(),
            vt_chunk_rx,
            reply_rx,
            control_rx,
            vt_cancel.clone(),
        ));

        Self {
            scrollback,
            event_sender,
            writer: Mutex::new(writer),
            master: Mutex::new(master),
            _child_killer: child_killer,
            vt_state,
            reply_tx,
            control_tx,
            frame_broadcast,
            frame_seq: AtomicU32::new(0),
            drop_counter,
            vt_chunk_tx,
            vt_cancel,
        }
    }

    /// Thin wrapper for the WS handler. The PTY bridge task does not go through
    /// this — it holds its own (scrollback, event_sender) clones.
    pub async fn snapshot_and_subscribe(&self) -> (Vec<u8>, Receiver<TerminalEvent>) {
        self.scrollback
            .snapshot_and_subscribe(&self.event_sender)
            .await
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        // Wake the VT bridge task so it exits its `tokio::select!` loop and
        // releases the cloned `Arc<Mutex<VtState>>`.
        self.vt_cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::TerminalEvent;
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

        // rx receives ONLY the new bytes, not the old ones (which are in snap).
        match rx.recv().await.unwrap() {
            TerminalEvent::Data { buffer } => assert_eq!(buffer, b"new"),
            other => panic!("unexpected event: {other:?}"),
        }
        // No more pending events.
        assert!(rx.try_recv().is_err());
    }
}
