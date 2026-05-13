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
    #[expect(
        dead_code,
        reason = "test-only helper; production goes through push_and_broadcast"
    )]
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

    /// Bundled VT state (Term + Parser + FrameRing + last_input_at), wrapped
    /// in `std::sync::Mutex` for short-held locks per PTY chunk in the bridge
    /// task. Read by `TerminalService::inspect_row` under the `test-helpers`
    /// feature.
    #[cfg_attr(
        not(any(test, feature = "test-helpers")),
        expect(dead_code, reason = "consumed by Phase 2 frame coalescer")
    )]
    pub(crate) vt_state: Arc<std::sync::Mutex<VtState>>,

    /// Reply-required events from `TermListener` (unbounded; must-not-drop
    /// to avoid capability-query backflow regression).
    #[expect(dead_code, reason = "consumed by Phase 2 frame coalescer")]
    pub(crate) reply_tx: mpsc::UnboundedSender<ReplyFrame>,

    /// Best-effort control events (bounded cap=64); try_send drops are
    /// rate-limited via `DropCounter`.
    #[expect(dead_code, reason = "consumed by Phase 2 frame coalescer")]
    pub(crate) control_tx: mpsc::Sender<ControlFrame>,

    /// Broadcast of MessagePack-encoded delta frames.
    #[expect(dead_code, reason = "consumed by Phase 2 frame coalescer")]
    pub(crate) frame_broadcast: broadcast::Sender<EncodedDelta>,

    /// Monotonic frame sequence number across reconnects (activity-scoped,
    /// resets on daemon restart).
    #[expect(dead_code, reason = "consumed by Phase 2 frame coalescer")]
    pub(crate) frame_seq: AtomicU32,

    /// Aggregated drop counter for bounded channel `try_send` failures.
    #[expect(dead_code, reason = "consumed by Phase 2 frame coalescer")]
    pub(crate) drop_counter: Arc<DropCounter>,

    /// Handle-side keepalive for the VT fan-out channel. The actual fan-out
    /// sender lives in `spawn_pty_reader`'s bridge task; holding a clone here
    /// keeps the channel alive for the lifetime of `PtyHandle` and reserves
    /// the slot for future direct injection (e.g., synthetic VT input).
    #[expect(
        dead_code,
        reason = "channel keepalive; direct producer added in Phase 2"
    )]
    pub(crate) vt_chunk_tx: mpsc::Sender<Bytes>,

    /// Cancellation for the VT bridge task; cancelled on `PtyHandle::drop`.
    pub(crate) vt_cancel: CancellationToken,
}

impl PtyHandle {
    #[expect(
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
        let vt_cancel = CancellationToken::new();

        // NOTE: the JoinHandle is intentionally discarded; cancellation via
        // vt_cancel on PtyHandle::drop is sufficient to terminate the task.
        tokio::spawn(run_bridge_task(
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

        match rx.recv().await.unwrap() {
            TerminalEvent::Data { buffer } => assert_eq!(buffer, b"new"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(rx.try_recv().is_err());
    }
}
