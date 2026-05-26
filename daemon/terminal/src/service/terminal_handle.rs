//! Per-activity bundle of PTY master/writer, scrollback, and VT bridge state.

use crate::event::TerminalEvent;
use crate::pty::scrollback::ScrollbackBuffer;
use crate::vt::bridge::{VtState, run_bridge_task};
use crate::vt::frame_ring::WireMessage;
use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
use crate::{PtyErrorBridge, TerminalError, TerminalGeometry, TerminalResult};
use alacritty_terminal::grid::Dimensions;
use bytes::Bytes;
use portable_pty::{ChildKiller, MasterPty, PtySize};
use std::sync::Mutex;
use std::{io::Write, sync::Arc};
use tokio::sync::{broadcast, mpsc};

pub(crate) struct TerminalHandle {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Handle-side sender for the VT fan-out channel. Used by
    /// `TerminalService::resize` to send a synthetic empty chunk that wakes
    /// the bridge task after `term.resize` sets `Full` damage.
    vt_chunk_tx: mpsc::Sender<Bytes>,
    event_sender: broadcast::Sender<TerminalEvent>,
    scrollback: ScrollbackBuffer,
    /// Kills the child process on Drop. Required because the PTY reader OS
    /// thread holds its own clone of `vt_chunk_tx` (so dropping this handle
    /// alone does not close the channel) and reads from a cloned master fd
    /// (so dropping `master` alone does not interrupt its blocking `read()`).
    /// Children that ignore SIGHUP (e.g. `nohup`) would otherwise keep the
    /// reader thread + bridge task + child tree alive indefinitely.
    child_killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,

    /// Bundled VT state (Term + Parser + FrameRing + pending_user_input),
    /// wrapped in `std::sync::Mutex` for short-held locks per PTY chunk in
    /// the bridge task. Read by `TerminalService::write`, `resize`,
    /// `subscribe_frames`, and `read_geometry`.
    pub(crate) vt_state: Arc<Mutex<VtState>>,

    /// Reply-required events from `TermListener` (unbounded; must-not-drop
    /// to avoid capability-query backflow regression). Held only to keep
    /// the channel open; `TermListener` owns the active clone and
    /// `reply_rx` is consumed by `run_bridge_task`.
    _reply_tx: mpsc::UnboundedSender<ReplyFrame>,

    /// Best-effort control events (bounded cap=64); try_send drops are
    /// rate-limited via `DropCounter`. Held only to keep the channel
    /// open; `TermListener` owns the active clone and `control_rx` is
    /// consumed by `run_bridge_task`.
    _control_tx: mpsc::Sender<ControlFrame>,

    /// Broadcast of wire messages (Binary deltas + Text sidecars). Held
    /// only to keep the channel open; the active `Sender` lives on
    /// `VtState::wire_broadcast` and subscribers attach via
    /// `TerminalService::subscribe_frames`.
    _frame_broadcast: broadcast::Sender<WireMessage>,

    /// Aggregated drop counter for bounded channel `try_send` failures.
    /// Held only to outlive any in-flight `try_send`; `TermListener` owns
    /// an `Arc` clone.
    _drop_counter: Arc<DropCounter>,
}

impl TerminalHandle {
    /// Construct a handle, spawning the VT bridge task on the current runtime.
    ///
    /// Called from `TerminalService::spawn` only.
    pub fn new(
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        event_sender: broadcast::Sender<TerminalEvent>,
        child_killer: Box<dyn ChildKiller + Send + Sync>,
        scrollback: ScrollbackBuffer,
        cols: u16,
        rows: u16,
        vt_chunk_tx: mpsc::Sender<Bytes>,
        vt_chunk_rx: mpsc::Receiver<Bytes>,
    ) -> Self {
        let (reply_tx, reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, control_rx) = mpsc::channel::<ControlFrame>(64);
        let (frame_broadcast, _frame_rx) = broadcast::channel::<WireMessage>(2048);
        let drop_counter = Arc::new(DropCounter::new());

        let listener = TermListener {
            reply_tx: reply_tx.clone(),
            control_tx: control_tx.clone(),
            drop_counter: drop_counter.clone(),
        };

        let vt_state = Arc::new(Mutex::new(VtState::new(
            cols,
            rows,
            listener,
            frame_broadcast.clone(),
        )));

        // NOTE: the JoinHandle is intentionally discarded; the bridge task
        // exits naturally when vt_chunk_rx is dropped (TerminalHandle::drop
        // drops vt_chunk_tx, closing the channel).
        tokio::spawn(run_bridge_task(
            vt_state.clone(),
            vt_chunk_rx,
            reply_rx,
            control_rx,
        ));

        Self {
            scrollback,
            event_sender,
            writer: Mutex::new(writer),
            master: Mutex::new(master),
            child_killer,
            vt_state,
            _reply_tx: reply_tx,
            _control_tx: control_tx,
            _frame_broadcast: frame_broadcast,
            _drop_counter: drop_counter,
            vt_chunk_tx,
        }
    }

    /// Returns a new broadcast receiver for `TerminalEvent` emissions.
    pub fn subscribe_events(&self) -> broadcast::Receiver<TerminalEvent> {
        self.event_sender.subscribe()
    }

    #[inline]
    pub fn write(&mut self, bytes: &[u8]) -> TerminalResult {
        // NOTE: flag is set BEFORE the PTY write so a racing bridge cycle
        // observing this user input cannot miss the flag — the bridge sees
        // either an empty PTY (no chunk yet, flag set) or a chunk plus flag.
        {
            let mut state = self.vt_state.lock().expect("vt_state poisoned");
            state.pending_user_input = true;
        }
        self.writer
            .get_mut()
            .unwrap()
            .write(bytes)
            .map_err(|e| TerminalError::Pty(e.to_string()))?;
        Ok(())
    }

    #[inline]
    pub fn resize(&mut self, size: PtySize) -> TerminalResult {
        {
            let dim = crate::vt::bridge::dim_for(size.cols, size.rows);
            let mut state = self.vt_state.lock().expect("vt_state poisoned");
            state.term.resize(dim);
            state.row_hashes.clear();
        }

        self.master
            .get_mut()
            .unwrap()
            .resize(size)
            .to_terminal_result()?;
        let _ = self.vt_chunk_tx.try_send(Bytes::new());

        Ok(())
    }

    #[inline]
    pub fn read_geometry(&self) -> TerminalGeometry {
        let vt_state = self.vt_state.lock().unwrap();
        TerminalGeometry {
            cols: vt_state.term.columns() as u16,
            rows: vt_state.term.screen_lines() as u16,
            cursor: crate::vt::frame_builder::extract_cursor(&vt_state.term),
        }
    }

    pub fn scroll(&mut self, delta: i32) {
        {
            let mut state = self.vt_state.lock().expect("vt_state poisoned");
            state
                .term
                .scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        }
        // NOTE: try_send rather than blocking send — matches resize semantics
        // so the wakeup is best-effort and cannot deadlock on a full channel.
        let _ = self.vt_chunk_tx.try_send(Bytes::new());
    }

    pub fn scroll_to_bottom(&mut self) {
        {
            let mut state = self.vt_state.lock().expect("vt_state poisoned");
            state
                .term
                .scroll_display(alacritty_terminal::grid::Scroll::Bottom);
        }
        // NOTE: try_send rather than blocking send — matches resize semantics
        // so the wakeup is best-effort and cannot deadlock on a full channel.
        let _ = self.vt_chunk_tx.try_send(Bytes::new());
    }

    /// Thin wrapper for the WS handler. The PTY bridge task does not go through
    /// this — it holds its own (scrollback, event_sender) clones.
    #[inline]
    pub fn snapshot(&self) -> Vec<u8> {
        self.scrollback.snapshot()
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        // SAFETY: Forces PTY hangup → OS reader thread's blocking `read()` returns
        // 0/Err → reader exits → its `vt_chunk_tx` clone drops → the bridge task's
        // `vt_chunk_rx.recv()` returns None → the bridge task exits cleanly.
        // Without this, children that ignore SIGHUP keep every link in that
        // chain alive (reader thread, bridge task, master fd, child process tree).
        if let Err(err) = self.child_killer.kill() {
            tracing::warn!(
                target: "ozmux_terminal::handle",
                ?err,
                "TerminalHandle::drop: child_killer.kill() failed (child may have already exited)"
            );
        }
    }
}
