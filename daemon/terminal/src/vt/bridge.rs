//! VtState + vt_bridge_task: drives Term from PTY chunks.
//!
//! Phase 1 advances `Term` only; frame emission and PtyWrite/control routing
//! are wired in Phase 2.

use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::vt::coalescer::{Coalescer, DamageVerdict};
use crate::vt::frame::{Cursor, RenderFrame, SnapshotReason, encode};
use crate::vt::frame_builder::{
    DirtyRows, build_delta, build_mode, build_snapshot, collect_dirty_rows, extract_cursor,
};
use crate::vt::frame_ring::{FrameRing, WireMessage};
use crate::vt::hyperlink::HyperlinkInterner;
use crate::vt::listener::{ControlFrame, ReplyFrame, TermListener};

/// All state mutated by the VT bridge task, wrapped by `PtyHandle` in
/// `std::sync::Mutex` so the bridge can take a short non-await lock per
/// PTY chunk.
pub struct VtState {
    /// Alacritty terminal model: grid, cursor, modes.
    pub term: Term<TermListener>,
    /// vte parser that drives `term` via `Processor::advance`.
    pub parser: alacritty_terminal::vte::ansi::Processor,
    /// Bounded ring of encoded delta frames available for replay on
    /// reconnect.
    pub frame_ring: FrameRing,
    /// One-shot flag set by [`crate::TerminalService::write`] when the
    /// client sends bytes to the PTY. Consumed by the bridge's coalescer
    /// on the first eligible emit (mirrors xterm.js's `_didUserInput`).
    pub pending_user_input: bool,
    /// Damage stashed by the bridge between the classify call and `emit_now`.
    /// The bridge reads `Term::damage()` exactly once per cycle so alacritty
    /// does not implicitly re-damage the cursor row.
    pub pending_damage: Option<DirtyRows>,
    /// Monotonic per-activity frame sequence number. Single-producer
    /// (bridge task) under the VtState lock.
    pub frame_seq: u32,
    /// Set false after the first frame emit so subsequent ones become deltas.
    pub first_emit: bool,
    /// Most recently emitted cursor state. Used to trigger cursor-only delta
    /// emits when the screen is otherwise idle (e.g., arrow-key motion that
    /// produces no dirty rows).
    pub prev_cursor: Option<Cursor>,
    /// OSC 8 hyperlink id interner. Maps alacritty `(id, uri)` pairs to
    /// monotonic u32 wire ids. Persists for the session — u32 wrapping
    /// (4G ids) exceeds any realistic session.
    pub hyperlinks: HyperlinkInterner,
    /// Broadcast of wire messages (Binary deltas + Text sidecars). All emit
    /// paths go through this sender; subscribers attach via subscribe_frames.
    pub wire_broadcast: broadcast::Sender<WireMessage>,
}

impl VtState {
    /// Constructs a fresh `VtState` with the given terminal dimensions.
    pub fn new(
        cols: u16,
        rows: u16,
        listener: TermListener,
        wire_broadcast: broadcast::Sender<WireMessage>,
    ) -> Self {
        let size = LocalDim {
            cols: cols.into(),
            rows: rows.into(),
        };
        let term = Term::new(Config::default(), &size, listener);
        Self {
            term,
            parser: alacritty_terminal::vte::ansi::Processor::new(),
            frame_ring: FrameRing::new(256 * 1024),
            pending_user_input: false,
            pending_damage: None,
            frame_seq: 0,
            first_emit: true,
            prev_cursor: None,
            hyperlinks: HyperlinkInterner::new(),
            wire_broadcast,
        }
    }

    /// Feeds a chunk of PTY bytes through `vte::Parser` into `Term`.
    ///
    /// Wrapped as a helper so the caller can borrow `parser` and `term`
    /// disjointly without tripping the borrow checker.
    pub fn advance(&mut self, chunk: &[u8]) {
        self.parser.advance(&mut self.term, chunk);
    }

    /// Returns true if the current `Term` cursor differs from the most recently
    /// emitted cursor (`prev_cursor`). Used by the bridge to drive cursor-only
    /// emit decisions and `AtMostOneRow` damage classification.
    pub fn cursor_changed(&self) -> bool {
        let curr = extract_cursor(&self.term);
        self.prev_cursor.as_ref().is_none_or(|prev| *prev != curr)
    }
}

/// Classifies the bridge's accumulated damage for the Coalescer's
/// immediate-flush decision. The cursor delta is folded in so that
/// cursor-only motion (no dirty rows) counts as `AtMostOneRow`.
fn classify_damage(dirty: &DirtyRows, cursor_changed: bool) -> DamageVerdict {
    match dirty {
        DirtyRows::Full => DamageVerdict::Full,
        DirtyRows::Rows(rows) if rows.is_empty() => {
            if cursor_changed {
                DamageVerdict::AtMostOneRow
            } else {
                DamageVerdict::Idle
            }
        }
        DirtyRows::Rows(rows) if rows.len() <= 1 => DamageVerdict::AtMostOneRow,
        DirtyRows::Rows(_) => DamageVerdict::ManyRows,
    }
}

/// Classification used by `decide_frame_kind` to select snapshot vs delta.
enum FrameKind {
    Snapshot { reason: SnapshotReason },
    Delta { rows: Vec<u16> },
}

/// Selects the appropriate frame type for the current emit based on damage state.
///
/// Policy, in priority order:
/// 1. `state.first_emit` → `Snapshot { reason: Initial }` (bootstrap frame).
/// 2. `DirtyRows::Full` → `Snapshot { reason: Resize }` (full damage after resize or clear).
/// 3. Partial damage ≥ 70 % of total rows → `Snapshot { reason: Resize }` (bandwidth crossover).
/// 4. Otherwise → `Delta { rows }`.
fn decide_frame_kind(state: &VtState, dirty: DirtyRows) -> FrameKind {
    let total_rows = state.term.screen_lines() as u16;
    if state.first_emit {
        return FrameKind::Snapshot {
            reason: SnapshotReason::Initial,
        };
    }
    match dirty {
        DirtyRows::Full => FrameKind::Snapshot {
            reason: SnapshotReason::Resize,
        },
        DirtyRows::Rows(rows) => {
            if (rows.len() as u32) * 10 >= (total_rows as u32) * 7 {
                FrameKind::Snapshot {
                    reason: SnapshotReason::Resize,
                }
            } else {
                FrameKind::Delta { rows }
            }
        }
    }
}

/// Maximum encoded frame size in bytes. Frames that exceed this are dropped and
/// replaced with an error text frame so the client can handle the anomaly.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Broadcasts an error text frame indicating the encoded frame exceeded
/// [`MAX_FRAME_BYTES`]. Increments the sequence number via the caller.
fn emit_frame_size_error(wb: &broadcast::Sender<WireMessage>, seq: u32) {
    let json = serde_json::json!({
        "kind": "error",
        "seq": seq,
        "category": "frame_size_exceeded",
    });
    let _ = wb.send(WireMessage::Text(json.to_string()));
}

/// Emits a frame for the damage stashed on `VtState` and disarms the
/// Coalescer. Called by [`run_bridge_task`] from both the chunk-immediate-flush
/// path and the deadline-fires path. The `window_open_mode` is consumed via
/// `.take()` so the next window starts with a fresh capture.
fn emit_now(
    vt_state: &Arc<std::sync::Mutex<VtState>>,
    coalescer: &mut Coalescer,
    window_open_mode: &mut Option<alacritty_terminal::term::TermMode>,
) {
    let mut state = vt_state.lock().expect("vt_state poisoned");

    let Some(dirty) = state.pending_damage.take() else {
        coalescer.disarm();
        *window_open_mode = None;
        return;
    };

    let prev_mode = window_open_mode
        .take()
        .unwrap_or_else(|| *state.term.mode());
    let curr_mode = *state.term.mode();
    let curr_cursor = extract_cursor(&state.term);
    let cursor_unchanged = state
        .prev_cursor
        .as_ref()
        .is_some_and(|prev| *prev == curr_cursor);

    let dirty_is_empty = matches!(&dirty, DirtyRows::Rows(r) if r.is_empty());
    if dirty_is_empty && prev_mode == curr_mode && cursor_unchanged && !state.first_emit {
        coalescer.disarm();
        return;
    }

    let kind = decide_frame_kind(&state, dirty);
    state.first_emit = false;
    let seq = state.frame_seq;
    let frame = {
        let VtState {
            ref term,
            ref mut hyperlinks,
            ..
        } = *state;
        match kind {
            FrameKind::Snapshot { reason } => {
                RenderFrame::Snapshot(build_snapshot(term, seq, reason, hyperlinks))
            }
            FrameKind::Delta { rows } => {
                RenderFrame::Delta(build_delta(term, seq, &rows, hyperlinks))
            }
        }
    };
    state.prev_cursor = Some(curr_cursor);
    state.term.reset_damage();
    let encoded_vec = encode(&frame).expect("encode infallible");

    if encoded_vec.len() > MAX_FRAME_BYTES {
        emit_frame_size_error(&state.wire_broadcast, state.frame_seq);
        state.frame_seq = state.frame_seq.wrapping_add(1);
        // NOTE: offending frame is dropped here; no ring push, no Binary broadcast.
    } else {
        let binary_seq = state.frame_seq;
        state.frame_seq = state.frame_seq.wrapping_add(1);
        let encoded = Bytes::from(encoded_vec);
        state.frame_ring.push(binary_seq, encoded.clone());

        // NOTE: mode is announced BEFORE the binary so the client
        // applies mode-related side-effects before re-rendering.
        if let Some(m) = build_mode(prev_mode, curr_mode, state.frame_seq) {
            state.frame_seq = state.frame_seq.wrapping_add(1);
            let json = serde_json::to_string(&m).expect("mode json infallible");
            let _ = state.wire_broadcast.send(WireMessage::Text(json));
        }
        let _ = state.wire_broadcast.send(WireMessage::Binary {
            seq: binary_seq,
            encoded,
        });
    }

    coalescer.disarm();
}

/// Drives the VT bridge: drains PTY chunks into `Term` via `vte::Parser` and
/// emits wire frames via the per-bridge [`Coalescer`].
///
/// `state.advance(chunk)` runs on every received chunk — the bounded
/// `pty_rx` channel uses `try_send` with silent drop, so any delay in
/// parsing risks data loss that is unrecoverable from the wire log.
/// The Coalescer only buffers the *decision to emit*; the Term itself
/// stays continuously up to date.
pub async fn run_bridge_task(
    vt_state: Arc<std::sync::Mutex<VtState>>,
    mut pty_rx: mpsc::Receiver<Bytes>,
    mut reply_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut control_rx: mpsc::Receiver<ControlFrame>,
    cancel: CancellationToken,
) {
    let mut coalescer = Coalescer::new();
    let mut window_open_mode: Option<alacritty_terminal::term::TermMode> = None;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = coalescer.wait_deadline() => {
                // NOTE: drain any chunks already queued at the moment the
                // deadline fires so they fold into the same emit. Single
                // lock spans the drain so damage is collected once at the
                // end (alacritty's damage state is cumulative across
                // advance calls until reset_damage).
                {
                    let mut state = vt_state.lock().expect("vt_state poisoned");
                    let pre_advance_mode = *state.term.mode();
                    while let Ok(chunk) = pty_rx.try_recv() {
                        if !chunk.is_empty() {
                            state.advance(&chunk);
                        }
                    }
                    if window_open_mode.is_none() {
                        window_open_mode = Some(pre_advance_mode);
                    }
                    state.pending_damage = Some(collect_dirty_rows(&mut state.term));
                }
                emit_now(&vt_state, &mut coalescer, &mut window_open_mode);
            }
            chunk = pty_rx.recv() => {
                let Some(chunk) = chunk else { break };
                let should_flush = {
                    let mut state = vt_state.lock().expect("vt_state poisoned");
                    let pre_advance_mode = *state.term.mode();
                    if !chunk.is_empty() {
                        state.advance(&chunk);
                    }
                    if !coalescer.is_armed() && window_open_mode.is_none() {
                        window_open_mode = Some(pre_advance_mode);
                    }
                    let dirty = collect_dirty_rows(&mut state.term);
                    let verdict = classify_damage(&dirty, state.cursor_changed());
                    let flush = coalescer.should_flush_immediately(
                        state.first_emit,
                        &verdict,
                        state.pending_user_input,
                    );
                    if flush
                        && state.pending_user_input
                        && matches!(verdict, DamageVerdict::AtMostOneRow)
                    {
                        state.pending_user_input = false;
                    }
                    state.pending_damage = Some(dirty);
                    flush
                };
                if should_flush {
                    emit_now(&vt_state, &mut coalescer, &mut window_open_mode);
                } else {
                    coalescer.arm_or_extend(Instant::now());
                }
            }
            reply = reply_rx.recv() => {
                if reply.is_none() {
                    break;
                }
            }
            ctrl = control_rx.recv() => {
                if ctrl.is_none() {
                    break;
                }
            }
        }
    }
}

// NOTE: minimal local `Dimensions` impl since alacritty's `TermSize` is
// `pub(crate)` inside its own crate.
pub(crate) struct LocalDim {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

impl Dimensions for LocalDim {
    fn columns(&self) -> usize {
        self.cols
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn total_lines(&self) -> usize {
        self.rows
    }
}

/// Constructs a [`LocalDim`] from terminal column/row counts.
///
/// Used by [`crate::pty::pty.rs`] `TerminalService::resize` to resize the
/// alacritty `Term` before resizing the PTY master.
pub(crate) fn dim_for(cols: u16, rows: u16) -> LocalDim {
    LocalDim {
        cols: cols.into(),
        rows: rows.into(),
    }
}

#[cfg(test)]
pub(crate) fn test_dim(cols: u16, rows: u16) -> LocalDim {
    dim_for(cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame};
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc};

    fn make_listener() -> TermListener {
        let (reply_tx, _) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _) = mpsc::channel::<ControlFrame>(64);
        TermListener {
            reply_tx,
            control_tx,
            drop_counter: Arc::new(DropCounter::new()),
        }
    }

    fn make_state(cols: u16, rows: u16) -> VtState {
        let (wire_broadcast, _rx) = broadcast::channel::<WireMessage>(256);
        VtState::new(cols, rows, make_listener(), wire_broadcast)
    }

    #[test]
    fn vt_state_constructs_with_dimensions() {
        let state = make_state(80, 24);
        assert!(state.frame_ring.is_empty());
        assert!(!state.pending_user_input);
        assert_eq!(state.frame_seq, 0);
        assert!(state.first_emit);
        assert!(state.prev_cursor.is_none());
        assert_eq!(state.term.columns(), 80);
        assert_eq!(state.term.screen_lines(), 24);
        assert!(state.pending_damage.is_none());
    }

    use bytes::Bytes;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn bridge_task_consumes_pty_chunks_and_updates_term() {
        let (reply_tx, reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, control_rx) = mpsc::channel::<ControlFrame>(64);
        let drop_counter = Arc::new(DropCounter::new());
        let listener = TermListener {
            reply_tx,
            control_tx,
            drop_counter: drop_counter.clone(),
        };
        let (wire_broadcast, _rx) = broadcast::channel::<WireMessage>(8);
        let vt_state = Arc::new(std::sync::Mutex::new(VtState::new(
            10,
            3,
            listener,
            wire_broadcast,
        )));

        let (pty_tx, pty_rx) = mpsc::channel::<Bytes>(8);
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(super::run_bridge_task(
            vt_state.clone(),
            pty_rx,
            reply_rx,
            control_rx,
            cancel.clone(),
        ));

        pty_tx.send(Bytes::from_static(b"hello")).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let line0_text: String = {
            let state = vt_state.lock().unwrap();
            let row = &state.term.grid()[alacritty_terminal::index::Line(0)];
            let slice =
                &row[alacritty_terminal::index::Column(0)..alacritty_terminal::index::Column(5)];
            slice.iter().map(|cell| cell.c).collect()
        };
        assert!(
            line0_text.starts_with("hello"),
            "expected 'hello' on row 0, got: {line0_text:?}"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn wire_broadcast_is_subscribable() {
        let (wire_broadcast, mut rx) = broadcast::channel::<WireMessage>(8);
        let state = VtState::new(10, 3, make_listener(), wire_broadcast.clone());
        let _ = state.wire_broadcast.send(WireMessage::Text("hello".into()));
        match rx.recv().await.unwrap() {
            WireMessage::Text(s) => assert_eq!(s, "hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn emit_frame_size_error_sends_text_with_category() {
        let (wb, mut rx) = broadcast::channel::<WireMessage>(16);
        super::emit_frame_size_error(&wb, 42);
        let msg = rx.recv().await.unwrap();
        match msg {
            WireMessage::Text(s) => {
                assert!(s.contains("\"kind\":\"error\""));
                assert!(s.contains("\"category\":\"frame_size_exceeded\""));
                assert!(s.contains("\"seq\":42"));
            }
            _ => panic!("expected Text(error)"),
        }
    }
}
