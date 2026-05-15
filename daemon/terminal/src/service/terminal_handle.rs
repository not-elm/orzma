//! Per-activity bundle of PTY master/writer, scrollback, and VT bridge state.

use crate::event::TerminalEvent;
use crate::pty::scrollback::ScrollbackBuffer;
use crate::vt::bridge::{VtState, run_bridge_task};
use crate::vt::frame_ring::WireMessage;
use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
use bytes::Bytes;
use portable_pty::{ChildKiller, MasterPty};
use std::{io::Write, sync::Arc};
use tokio::sync::{
    Mutex, broadcast,
    broadcast::{Receiver, Sender},
    mpsc,
};
use tokio_util::sync::CancellationToken;

pub(crate) struct TerminalHandle {
    pub master: Mutex<Box<dyn MasterPty + Send>>,
    pub writer: Mutex<Box<dyn Write + Send>>,
    pub scrollback: ScrollbackBuffer,
    pub event_sender: Sender<TerminalEvent>,
    _child_killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,

    /// Bundled VT state (Term + Parser + FrameRing + pending_user_input),
    /// wrapped in `std::sync::Mutex` for short-held locks per PTY chunk in
    /// the bridge task. Read by `TerminalService::write`, `resize`,
    /// `subscribe_frames`, `read_geometry`, and the
    /// `cfg(any(test, feature = "test-helpers"))` helpers (`inspect_row`,
    /// `inspect_damage_and_reset`, `peek_pending_user_input`) in
    /// `service/test_helpers.rs`.
    pub(crate) vt_state: Arc<std::sync::Mutex<VtState>>,

    /// Reply-required events from `TermListener` (unbounded; must-not-drop
    /// to avoid capability-query backflow regression).
    #[expect(
        dead_code,
        reason = "held for the lifetime of TermListener (which owns a clone); \
                  reply_rx is consumed by run_bridge_task"
    )]
    pub(crate) reply_tx: mpsc::UnboundedSender<ReplyFrame>,

    /// Best-effort control events (bounded cap=64); try_send drops are
    /// rate-limited via `DropCounter`.
    #[expect(
        dead_code,
        reason = "held for the lifetime of TermListener (which owns a clone); \
                  control_rx is consumed by run_bridge_task"
    )]
    pub(crate) control_tx: mpsc::Sender<ControlFrame>,

    /// Broadcast of wire messages (Binary deltas + Text sidecars).
    #[expect(
        dead_code,
        reason = "held to keep the broadcast channel open; the active Sender \
                  lives on VtState::wire_broadcast and subscribers attach via \
                  TerminalService::subscribe_frames"
    )]
    pub(crate) frame_broadcast: broadcast::Sender<WireMessage>,

    /// Aggregated drop counter for bounded channel `try_send` failures.
    #[expect(
        dead_code,
        reason = "held for the lifetime of TermListener (which owns an Arc clone) \
                  so the counter outlives any in-flight try_send"
    )]
    pub(crate) drop_counter: Arc<DropCounter>,

    /// Handle-side sender for the VT fan-out channel. Used by
    /// `TerminalService::resize` to send a synthetic empty chunk that wakes
    /// the bridge task after `term.resize` sets `Full` damage.
    pub(crate) vt_chunk_tx: mpsc::Sender<Bytes>,

    /// Cancellation for the VT bridge task; cancelled on `TerminalHandle::drop`.
    pub(crate) vt_cancel: CancellationToken,
}

impl TerminalHandle {
    /// Construct a handle, spawning the VT bridge task on the current runtime.
    ///
    /// Called from `TerminalService::spawn` only.
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor wires raw PTY + VT bridge resources; a builder \
                  would obscure the single call site in TerminalService::spawn"
    )]
    pub(crate) fn new(
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
        let (frame_broadcast, _frame_rx) = broadcast::channel::<WireMessage>(256);
        let drop_counter = Arc::new(DropCounter::new());

        let listener = TermListener {
            reply_tx: reply_tx.clone(),
            control_tx: control_tx.clone(),
            drop_counter: drop_counter.clone(),
        };

        let vt_state = Arc::new(std::sync::Mutex::new(VtState::new(
            cols,
            rows,
            listener,
            frame_broadcast.clone(),
        )));
        let vt_cancel = CancellationToken::new();

        // NOTE: the JoinHandle is intentionally discarded; cancellation via
        // vt_cancel on TerminalHandle::drop is sufficient to terminate the task.
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
            drop_counter,
            vt_chunk_tx,
            vt_cancel,
        }
    }

    /// Thin wrapper for the WS handler. The PTY bridge task does not go through
    /// this — it holds its own (scrollback, event_sender) clones.
    pub(crate) async fn snapshot_and_subscribe(&self) -> (Vec<u8>, Receiver<TerminalEvent>) {
        self.scrollback
            .snapshot_and_subscribe(&self.event_sender)
            .await
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        self.vt_cancel.cancel();
    }
}
