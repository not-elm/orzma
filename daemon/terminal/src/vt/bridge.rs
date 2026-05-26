//! VtState + vt_bridge_task: drives Term from PTY chunks.
//!
//! Phase 1 advances `Term` only; frame emission and PtyWrite/control routing
//! are wired in Phase 2.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

use crate::vt::coalescer::{Coalescer, DamageVerdict};
use crate::vt::frame::{Cursor, CursorShape, RenderFrame, SnapshotReason, encode};
use crate::vt::frame_builder::{
    DirtyRows, build_delta, build_mode, build_snapshot, collect_dirty_rows, extract_cursor,
    viewport_row_to_line,
};
use crate::vt::frame_ring::{FrameRing, WireMessage};
use crate::vt::hyperlink::HyperlinkInterner;
use crate::vt::listener::{ControlFrame, ReplyFrame, TermListener};
use crate::vt::title::sanitize_title;

/// All state mutated by the VT bridge task, wrapped by `TerminalHandle` in
/// `std::sync::Mutex` so the bridge can take a short non-await lock per
/// PTY chunk.
pub(crate) struct VtState {
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
    /// Most recent sanitized OSC terminal title, or `None` before any
    /// title has been set / after `ResetTitle`. Read by
    /// `TerminalService::all_titles` for tab labels.
    pub title: Option<String>,
    /// Reusable scratch buffer for dirty row indices collected from
    /// `Term::damage()`. Lives on `VtState` so the heap allocation
    /// persists across emit cycles instead of being freshly allocated
    /// on each call to `collect_dirty_rows`.
    pub(crate) scratch_dirty: Vec<u16>,
    /// Per-grid-line content hash captured at the time of the most recent
    /// emit. The bridge filters `DirtyRows::Rows` against these hashes
    /// before `decide_frame_kind` so cosmetic re-damage of unchanged rows
    /// (e.g., alacritty's cursor-row implicit re-damage) does not inflate
    /// the row count past the snapshot threshold or burn coalesce wins on
    /// no-op deltas. Keyed by alacritty `Line(i32)` so scrollback rows
    /// (negative indices) can be tracked too.
    pub(crate) row_hashes: HashMap<i32, u64>,
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
            title: None,
            scratch_dirty: Vec::new(),
            row_hashes: HashMap::new(),
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
        DirtyRows::Rows(rows) => DamageVerdict::ManyRows { rows: rows.len() },
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
/// 3. Partial damage ≥ 85 % of total rows → `Snapshot { reason: Resize }` (bandwidth crossover).
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
            if (rows.len() as u32) * 20 >= (total_rows as u32) * 17 {
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

/// Hashes a vte `NamedColor` discriminant into `h`.
///
/// vte `NamedColor` derives `Ord`/`PartialOrd` but NOT `Hash`. Discriminants
/// are explicit (Foreground=256, etc.), so the `u32` cast preserves identity.
fn hash_named_color(n: NamedColor, h: &mut DefaultHasher) {
    (n as u32).hash(h);
}

/// Hashes a vte `Rgb` triple into `h` by walking its public fields, since
/// `Rgb` derives `PartialEq`/`Default` but NOT `Hash`.
fn hash_rgb(rgb: &Rgb, h: &mut DefaultHasher) {
    rgb.r.hash(h);
    rgb.g.hash(h);
    rgb.b.hash(h);
}

/// Hashes a vte `Color` (`AColor`) value into `h` by walking the
/// discriminant and payload, since `AColor` does NOT derive `Hash`.
fn hash_acolor(c: &AColor, h: &mut DefaultHasher) {
    match c {
        AColor::Named(n) => {
            0u8.hash(h);
            hash_named_color(*n, h);
        }
        AColor::Spec(rgb) => {
            1u8.hash(h);
            hash_rgb(rgb, h);
        }
        AColor::Indexed(i) => {
            2u8.hash(h);
            i.hash(h);
        }
    }
}

/// Hashes an ozmux wire `CursorShape` into `h` by mapping the variant to a
/// `u8`, since the wire type does NOT derive `Hash`.
fn hash_cursor_shape(s: CursorShape, h: &mut DefaultHasher) {
    let n: u8 = match s {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
    };
    n.hash(h);
}

/// Computes a content hash for a single grid row, including the cursor
/// overlay if the cursor is visible and rendered on this `viewport_y`.
fn hash_row<T>(term: &Term<T>, line: Line, cursor: &Cursor, viewport_y: u16) -> u64 {
    let mut h = DefaultHasher::new();
    for cell in &term.grid()[line] {
        cell.c.hash(&mut h);
        hash_acolor(&cell.fg, &mut h);
        hash_acolor(&cell.bg, &mut h);
        cell.flags.bits().hash(&mut h);
        if let Some(hyp) = cell.hyperlink().as_ref() {
            1u8.hash(&mut h);
            hyp.uri().hash(&mut h);
            hyp.id().hash(&mut h);
        } else {
            0u8.hash(&mut h);
        }
    }
    if cursor.visible && viewport_y == cursor.y {
        cursor.x.hash(&mut h);
        cursor.y.hash(&mut h);
        hash_cursor_shape(cursor.shape, &mut h);
        cursor.blinking.hash(&mut h);
    }
    h.finish()
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

    let Some(mut dirty) = state.pending_damage.take() else {
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

    // NOTE: hash-filter DirtyRows::Rows BEFORE decide_frame_kind — otherwise
    // the row-count threshold check false-promotes unchanged rows to Snapshot.
    let mut kept_hashes: Vec<(i32, u64)> = Vec::new();
    if let DirtyRows::Rows(rows) = &mut dirty {
        let VtState {
            ref term,
            ref row_hashes,
            ..
        } = *state;
        rows.retain(|&viewport_y| {
            let line = viewport_row_to_line(term, viewport_y as i32);
            let h = hash_row(term, line, &curr_cursor, viewport_y);
            let stale = row_hashes.get(&line.0).copied();
            if Some(h) == stale {
                false
            } else {
                kept_hashes.push((line.0, h));
                true
            }
        });
    }

    let dirty_is_empty = matches!(&dirty, DirtyRows::Rows(r) if r.is_empty());
    if dirty_is_empty && prev_mode == curr_mode && cursor_unchanged && !state.first_emit {
        // Reset alacritty's damage tracker so we don't re-walk the same
        // damage on the next coalescer flush.
        state.term.reset_damage();
        coalescer.disarm();
        return;
    }

    let kind = decide_frame_kind(&state, dirty);
    state.first_emit = false;
    let seq = state.frame_seq;

    // NOTE: we track the consumed rows Vec from FrameKind::Delta so capacity
    // can be reclaimed into scratch_dirty after the emit completes.
    let mut consumed_rows: Option<Vec<u16>> = None;
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
                let frame = RenderFrame::Delta(build_delta(term, seq, &rows, hyperlinks));
                consumed_rows = Some(rows);
                frame
            }
        }
    };
    state.prev_cursor = Some(curr_cursor);
    state.term.reset_damage();
    let encoded_vec = encode(&frame).expect("encode infallible");

    if encoded_vec.len() > MAX_FRAME_BYTES {
        emit_frame_size_error(&state.wire_broadcast, state.frame_seq);
        // NOTE: frame_seq still advances on drop so the wire reflects a gap;
        // clients use the gap to know a frame was lost rather than silently
        // skipping seq numbers.
        state.frame_seq = state.frame_seq.wrapping_add(1);
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

        match &frame {
            RenderFrame::Delta(_) => {
                for (line_i32, h) in kept_hashes {
                    state.row_hashes.insert(line_i32, h);
                }
            }
            RenderFrame::Snapshot(_) => {
                state.row_hashes.clear();
                let screen_rows = state.term.grid().screen_lines() as u16;
                let snap_cursor = extract_cursor(&state.term);
                let VtState {
                    ref term,
                    ref mut row_hashes,
                    ..
                } = *state;
                for viewport_y in 0..screen_rows {
                    let line = viewport_row_to_line(term, viewport_y as i32);
                    let h = hash_row(term, line, &snap_cursor, viewport_y);
                    row_hashes.insert(line.0, h);
                }
            }
        }
    }

    if let Some(v) = consumed_rows {
        state.scratch_dirty = v;
    }

    coalescer.disarm();
}

/// Drives the VT bridge: drains PTY chunks into `Term` via `vte::Parser` and
/// emits wire frames via the per-bridge [`Coalescer`].
///
/// `state.advance(chunk)` runs on every received chunk. The bounded
/// `pty_rx` channel is fed by the PTY reader OS thread via `blocking_send`,
/// so bursty PTY output applies backpressure to the read thread rather than
/// dropping bytes — the VT parser never misses a chunk. The Coalescer only
/// buffers the *decision to emit*; the Term itself stays continuously up to
/// date.
///
/// `vt_chunk_tx::try_send` is still used by `TerminalHandle::resize` and
/// `scroll` to send synthetic empty-byte wakeups — those are best-effort
/// signals where a drop only means a slightly delayed snapshot, not bytes
/// missing from the Term.
pub(crate) async fn run_bridge_task(
    vt_state: Arc<Mutex<VtState>>,
    mut pty_rx: mpsc::Receiver<Bytes>,
    mut reply_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut control_rx: mpsc::Receiver<ControlFrame>,
) {
    let mut coalescer = Coalescer::new();
    let mut window_open_mode: Option<alacritty_terminal::term::TermMode> = None;

    loop {
        tokio::select! {
            biased;
            () = coalescer.wait_deadline() => {
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
                    let mut scratch = std::mem::take(&mut state.scratch_dirty);
                    state.pending_damage = Some(collect_dirty_rows(&mut state.term, &mut scratch));
                    state.scratch_dirty = scratch;
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
                    let mut scratch = std::mem::take(&mut state.scratch_dirty);
                    let dirty = collect_dirty_rows(&mut state.term, &mut scratch);
                    state.scratch_dirty = scratch;
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
                match ctrl {
                    None => break,
                    Some(ControlFrame::Title(title)) => {
                        let mut state = vt_state.lock().expect("vt_state poisoned");
                        state.title = Some(sanitize_title(&title));
                    }
                    Some(ControlFrame::ResetTitle) => {
                        let mut state = vt_state.lock().expect("vt_state poisoned");
                        state.title = None;
                    }
                    // NOTE: Bell and Clipboard remain intentionally dropped.
                    Some(ControlFrame::Bell | ControlFrame::Clipboard { .. }) => {}
                }
            }
        }
    }
}

// NOTE: minimal local `Dimensions` impl since alacritty's `TermSize` is
// `pub(crate)` inside its own crate.
pub(crate) struct LocalDim {
    cols: usize,
    rows: usize,
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

/// Test helper alias for [`dim_for`].
#[cfg(test)]
pub(crate) fn test_dim(cols: u16, rows: u16) -> LocalDim {
    dim_for(cols, rows)
}
