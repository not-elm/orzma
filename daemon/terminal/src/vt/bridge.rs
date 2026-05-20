//! VtState + vt_bridge_task: drives Term from PTY chunks.
//!
//! Phase 1 advances `Term` only; frame emission and PtyWrite/control routing
//! are wired in Phase 2.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};
use bytes::Bytes;
use metrics::{counter, gauge};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::vt::coalescer::{Coalescer, DamageVerdict};
use crate::vt::frame::{Cursor, CursorShape, RenderFrame, SnapshotReason, encode};
use crate::vt::frame_builder::{
    DirtyRows, build_delta, build_snapshot, collect_dirty_rows, extract_cursor,
    viewport_row_to_line,
};
use crate::vt::frame_ring::{FrameRing, WireMessage};
use crate::vt::hyperlink::HyperlinkInterner;
use crate::vt::listener::{ControlFrame, ReplyFrame, TermListener};
use crate::vt::produced_at::produced_at_enabled;
use crate::vt::title::sanitize_title;
use ozmux_multiplexer::{ActivityId, WindowId};

/// Internal-only knob for replay/test contexts. Production code uses
/// `BridgeConfig::default()` (unchanged behavior).
///
/// See `docs/superpowers/specs/2026-05-19-pr-a-replay-harness-design.md`
/// Section 3 "BridgeConfig".
#[derive(Debug, Clone, Copy)]
pub(crate) struct BridgeConfig {
    /// Whether the coalescer's IDLE/MAX_CAP windows are honored.
    /// False = flush-after-each-chunk (replay determinism mode).
    pub coalesce: bool,
    /// Whether to spawn the 100 ms broadcast-depth gauge task alongside
    /// the bridge. False = no gauge (replay determinism: no extra timers
    /// that would interfere with `tokio::time::pause()`).
    pub spawn_gauge: bool,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            coalesce: true,
            spawn_gauge: true,
        }
    }
}

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
    /// Monotonic wall-clock captured at bridge construction.
    /// Used together with `bridge_started_at_unix_us` to derive epoch
    /// micros for each emitted frame without a `getenv`-per-frame syscall.
    /// `std::time::Instant` is used rather than `tokio::time::Instant`
    /// because benchmarks run with `Builder::start_paused(true)`, which
    /// freezes tokio time and would make elapsed() return ~0.
    pub(crate) started_at: std::time::Instant,
    /// Wall-clock epoch micros captured at the same moment as `started_at`.
    /// `None` when `SystemTime` predates the Unix epoch (essentially never).
    pub(crate) bridge_started_at_unix_us: Option<u64>,
    /// Modes that became active since the previous successful emit.
    /// OVERWRITTEN on each `term.advance(chunk)` call — never appended —
    /// to keep the net transition correct under A→B→A flips.
    /// `&'static str` (not `String`) avoids per-chunk allocation; converted
    /// to `Vec<String>` only at emit-time when building the FrameDelta.
    pub(crate) pending_modes_added: Vec<&'static str>,
    /// Modes that became inactive since the previous successful emit.
    pub(crate) pending_modes_removed: Vec<&'static str>,
    /// Mirror of `term.mode()` captured at the previous successful emit.
    /// Initialized in `VtState::new` to `*term.mode()` (alacritty default
    /// mode set at construction time).
    pub(crate) last_emit_mode: alacritty_terminal::term::TermMode,
    /// Per-grid-Line content hash from the previous emit. Keyed by absolute
    /// `Line` inner i32 (negative for history rows). Used by CAT-005 to
    /// drop unchanged rows from DirtyRows::Rows before they reach
    /// decide_frame_kind. Bulk-reset on every Snapshot emit and on resize.
    pub(crate) row_hashes: HashMap<i32, u64>,
    /// Reusable scratch buffer for collect_dirty_rows. Cleared and re-filled
    /// per emit. Capacity is preserved across emits via the move-and-reclaim
    /// pattern (the variant takes ownership; we write it back after consumption).
    pub(crate) scratch_dirty: Vec<u16>,
    /// Handle for the 100 ms broadcast-depth gauge task.
    /// `AbortOnDropHandle` cancels the task when this `VtState` is dropped.
    /// `None` when `BridgeConfig::spawn_gauge` is false.
    gauge_handle: Option<AbortOnDropHandle<()>>,
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
        let last_emit_mode = *term.mode();
        let started_at = std::time::Instant::now();
        let bridge_started_at_unix_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| d.as_micros().try_into().ok());
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
            started_at,
            bridge_started_at_unix_us,
            pending_modes_added: Vec::new(),
            pending_modes_removed: Vec::new(),
            last_emit_mode,
            row_hashes: HashMap::new(),
            scratch_dirty: Vec::new(),
            gauge_handle: None,
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

    /// Returns the wall-clock epoch micros at which the current frame is
    /// being emitted, or `None` if produced-at observability is disabled or
    /// the bridge could not capture a wall-clock origin at construction.
    fn current_produced_at_us(&self) -> Option<u64> {
        if !produced_at_enabled() {
            return None;
        }
        let origin = self.bridge_started_at_unix_us?;
        let elapsed_us: u64 = self.started_at.elapsed().as_micros().try_into().ok()?;
        Some(origin.saturating_add(elapsed_us))
    }
}

fn hash_named_color(n: NamedColor, h: &mut DefaultHasher) {
    // NOTE: vte NamedColor derives Ord/PartialOrd but NOT Hash. Discriminants
    // are explicit (Foreground=256, etc.), so u32 cast preserves identity.
    (n as u32).hash(h);
}

fn hash_rgb(rgb: &Rgb, h: &mut DefaultHasher) {
    // NOTE: vte Rgb derives PartialEq/Default but NOT Hash. Hash public fields.
    rgb.r.hash(h);
    rgb.g.hash(h);
    rgb.b.hash(h);
}

fn hash_acolor(c: &AColor, h: &mut DefaultHasher) {
    // NOTE: vte Color (AColor) does NOT derive Hash; walk discriminant + payload.
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

fn hash_cursor_shape(s: CursorShape, h: &mut DefaultHasher) {
    // NOTE: ozmux wire CursorShape does NOT derive Hash — map to u8.
    let n: u8 = match s {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
    };
    n.hash(h);
}

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

/// Emits a frame for the damage stashed on `VtState` and disarms the
/// Coalescer. Called by [`run_bridge_task`] from both the chunk-immediate-flush
/// path and the deadline-fires path.
fn emit_now(vt_state: &Arc<std::sync::Mutex<VtState>>, coalescer: &mut Coalescer) {
    let mut state = vt_state.lock().expect("vt_state poisoned");

    let Some(mut dirty) = state.pending_damage.take() else {
        coalescer.disarm();
        return;
    };

    let curr_cursor = extract_cursor(&state.term);

    // CAT-005: hash-filter DirtyRows::Rows BEFORE decide_frame_kind so the
    // threshold check doesn't false-promote unchanged rows to Snapshot.
    // kept_hashes records (line_i32, hash) for rows that passed the filter;
    // written into row_hashes in the commit phase after successful encode.
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

    // CAT-005 + CAT-007: re-check emit eligibility after the filter.
    let has_dirty =
        matches!(&dirty, DirtyRows::Full) || matches!(&dirty, DirtyRows::Rows(r) if !r.is_empty());
    let has_pending_modes =
        !state.pending_modes_added.is_empty() || !state.pending_modes_removed.is_empty();
    let cursor_unchanged = state
        .prev_cursor
        .as_ref()
        .is_some_and(|prev| *prev == curr_cursor);

    if !has_dirty && !has_pending_modes && cursor_unchanged && !state.first_emit {
        // Truly nothing to emit. Reset alacritty's damage tracker so we
        // don't re-walk the same damage on the next coalescer flush.
        state.term.reset_damage();
        coalescer.disarm();
        return;
    }

    let kind = decide_frame_kind(&state, dirty);
    state.first_emit = false;
    let seq = state.frame_seq;
    let produced_at = state.current_produced_at_us();

    // CAT-007: convert pending mode transitions (Vec<&'static str>) to owned
    // Vec<String> for wire serialization. Cleared in the commit phase below.
    let modes_added_owned: Vec<String> = state
        .pending_modes_added
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let modes_removed_owned: Vec<String> = state
        .pending_modes_removed
        .iter()
        .map(|s| (*s).to_string())
        .collect();

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
                RenderFrame::Snapshot(build_snapshot(term, seq, reason, hyperlinks, produced_at))
            }
            FrameKind::Delta { rows } => {
                let frame = RenderFrame::Delta(build_delta(
                    term,
                    seq,
                    &rows,
                    hyperlinks,
                    produced_at,
                    modes_added_owned,
                    modes_removed_owned,
                ));
                consumed_rows = Some(rows);
                frame
            }
        }
    };
    state.prev_cursor = Some(curr_cursor);
    state.term.reset_damage();
    let encoded_vec = match encode(&frame) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("frame encode failed: {e}");
            coalescer.disarm();
            return;
        }
    };

    if encoded_vec.len() > MAX_FRAME_BYTES {
        let error_seq = state.frame_seq;
        // NOTE: frame_seq still advances on drop so the wire reflects a gap;
        // clients use the gap to know a frame was lost rather than silently
        // skipping seq numbers.
        state.frame_seq = state.frame_seq.wrapping_add(1);
        let json = serde_json::json!({
            "kind": "error",
            "seq": error_seq,
            "category": "frame_size_exceeded",
        })
        .to_string();
        state.frame_ring.push_error(error_seq, json.clone());
        let _ = state.wire_broadcast.send(WireMessage::Text(json));
    } else {
        let binary_seq = state.frame_seq;
        state.frame_seq = state.frame_seq.wrapping_add(1);
        let encoded = Bytes::from(encoded_vec);
        state.frame_ring.push_binary(binary_seq, encoded.clone());
        let _ = state.wire_broadcast.send(WireMessage::Binary {
            seq: binary_seq,
            encoded,
        });
        let kind_label = match &frame {
            RenderFrame::Snapshot(_) => "snapshot",
            RenderFrame::Delta(_) => "delta",
        };
        counter!("ozmux_frames_emit_total", "kind" => kind_label).increment(1);

        // CAT-007 commit phase: the pending mode transition has been bundled into
        // the wire frame. Reset pending and update last_emit_mode so the next
        // advance computes the diff from this point.
        state.pending_modes_added.clear();
        state.pending_modes_removed.clear();
        state.last_emit_mode = *state.term.mode();

        // CAT-005 commit phase: update row_hashes after successful encode + push.
        // For delta frames, write the kept (line, hash) pairs computed above.
        // For snapshot frames, bulk-rehash all visible rows as the new baseline.
        match &frame {
            RenderFrame::Delta(_) => {
                for (line_i32, h) in kept_hashes {
                    state.row_hashes.insert(line_i32, h);
                }
            }
            RenderFrame::Snapshot(_) => {
                // CAT-005: snapshot is a full reset point. Re-hash all visible rows
                // so the next delta can correctly identify changes.
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

    // CAT-006: reclaim the rows Vec capacity into scratch_dirty so the
    // allocator does not need to reallocate on the next collect_dirty_rows call.
    if let Some(v) = consumed_rows {
        state.scratch_dirty = v;
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
#[expect(
    clippy::too_many_arguments,
    reason = "bridge wires raw VT + PTY + control + title channels plus a \
              determinism config; a builder would obscure the single \
              hot-path call site in TerminalHandle::new"
)]
#[tracing::instrument(skip_all, fields(activity_id = tracing::field::Empty))]
pub(crate) async fn run_bridge_task(
    vt_state: Arc<std::sync::Mutex<VtState>>,
    mut pty_rx: mpsc::Receiver<Bytes>,
    mut reply_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut control_rx: mpsc::Receiver<ControlFrame>,
    activity_window: Option<WindowId>,
    title_tx: broadcast::Sender<WindowId>,
    cancel: CancellationToken,
    config: BridgeConfig,
    activity_id: ActivityId,
) {
    tracing::Span::current().record("activity_id", activity_id.as_ref());
    if config.spawn_gauge {
        let tx_for_gauge = vt_state
            .lock()
            .expect("vt_state poisoned")
            .wire_broadcast
            .clone();
        let depth_gauge = gauge!("ozmux_broadcast_queue_depth", "kind" => "terminal");
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                depth_gauge.set(tx_for_gauge.len() as f64);
            }
        });
        vt_state.lock().expect("vt_state poisoned").gauge_handle =
            Some(AbortOnDropHandle::new(handle));
    }
    let mut coalescer = Coalescer::new();

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            (_elapsed, _trigger) = coalescer.wait_deadline() => {
                // NOTE: drain any chunks already queued at the moment the
                // deadline fires so they fold into the same emit. Single
                // lock spans the drain so damage is collected once at the
                // end (alacritty's damage state is cumulative across
                // advance calls until reset_damage).
                {
                    let mut state = vt_state.lock().expect("vt_state poisoned");
                    while let Ok(chunk) = pty_rx.try_recv() {
                        if !chunk.is_empty() {
                            state.advance(&chunk);
                            // CAT-007: recompute mode transition against last_emit_mode. OVERWRITE
                            // semantics (never append) keep the net transition correct across A→B→A.
                            let curr_mode = *state.term.mode();
                            let diff = crate::vt::mode_diff::diff_mode(state.last_emit_mode, curr_mode);
                            state.pending_modes_added = diff.added;
                            state.pending_modes_removed = diff.removed;
                        }
                    }
                    let mut scratch = std::mem::take(&mut state.scratch_dirty);
                    state.pending_damage =
                        Some(collect_dirty_rows(&mut state.term, &mut scratch));
                    state.scratch_dirty = scratch;
                }
                emit_now(&vt_state, &mut coalescer);
            }
            chunk = pty_rx.recv() => {
                let Some(chunk) = chunk else { break };
                let should_flush = {
                    let mut state = vt_state.lock().expect("vt_state poisoned");
                    if !chunk.is_empty() {
                        state.advance(&chunk);
                        // CAT-007: recompute mode transition against last_emit_mode. OVERWRITE
                        // semantics (never append) keep the net transition correct across A→B→A.
                        let curr_mode = *state.term.mode();
                        let diff = crate::vt::mode_diff::diff_mode(state.last_emit_mode, curr_mode);
                        state.pending_modes_added = diff.added;
                        state.pending_modes_removed = diff.removed;
                    }
                    let mut scratch = std::mem::take(&mut state.scratch_dirty);
                    let dirty = collect_dirty_rows(&mut state.term, &mut scratch);
                    state.scratch_dirty = scratch;
                    let verdict = classify_damage(&dirty, state.cursor_changed());
                    let policy_flush = coalescer.should_flush_immediately(
                        state.first_emit,
                        &verdict,
                        state.pending_user_input,
                    );
                    // NOTE: replay/test mode disables the coalescer entirely
                    // so emits are 1:1 with PTY chunks — required by
                    // feed_pty_tape so output is reproducible without
                    // tokio::time::advance dances.
                    let flush = policy_flush || !config.coalesce;
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
                    emit_now(&vt_state, &mut coalescer);
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
                    Some(ControlFrame::Title(raw)) => {
                        // NOTE: shells re-emit the same OSC title on every prompt
                        // redraw; only signal when the value actually changed so a
                        // redraw storm cannot drive WindowView re-broadcasts.
                        let clean = sanitize_title(&raw);
                        let changed = {
                            let mut state = vt_state.lock().expect("vt_state poisoned");
                            let changed = state.title.as_deref() != Some(clean.as_str());
                            if changed {
                                state.title = Some(clean);
                            }
                            changed
                        };
                        if changed && let Some(wid) = &activity_window {
                            let _ = title_tx.send(wid.clone());
                        }
                    }
                    Some(ControlFrame::ResetTitle) => {
                        let changed = {
                            let mut state = vt_state.lock().expect("vt_state poisoned");
                            let changed = state.title.is_some();
                            state.title = None;
                            changed
                        };
                        if changed && let Some(wid) = &activity_window {
                            let _ = title_tx.send(wid.clone());
                        }
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

#[cfg(test)]
pub(super) fn test_dim(cols: u16, rows: u16) -> LocalDim {
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
        let (title_tx, _title_rx) = broadcast::channel::<ozmux_multiplexer::WindowId>(8);
        let handle = tokio::spawn(super::run_bridge_task(
            vt_state.clone(),
            pty_rx,
            reply_rx,
            control_rx,
            None,
            title_tx,
            cancel.clone(),
            BridgeConfig::default(),
            ActivityId::new(),
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

    struct TitleBridge {
        control_tx: mpsc::Sender<ControlFrame>,
        title_rx: broadcast::Receiver<ozmux_multiplexer::WindowId>,
        vt_state: Arc<std::sync::Mutex<VtState>>,
        wid: ozmux_multiplexer::WindowId,
        cancel: CancellationToken,
        handle: tokio::task::JoinHandle<()>,
        // NOTE: kept alive so the bridge's pty_rx does not see a closed channel.
        _pty_tx: mpsc::Sender<Bytes>,
    }

    fn spawn_title_bridge() -> TitleBridge {
        let (reply_tx, reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, control_rx) = mpsc::channel::<ControlFrame>(64);
        let drop_counter = Arc::new(DropCounter::new());
        let listener = TermListener {
            reply_tx,
            control_tx: control_tx.clone(),
            drop_counter,
        };
        let (wire_broadcast, _rx) = broadcast::channel::<WireMessage>(8);
        let vt_state = Arc::new(std::sync::Mutex::new(VtState::new(
            10,
            3,
            listener,
            wire_broadcast,
        )));
        let (pty_tx, pty_rx) = mpsc::channel::<Bytes>(8);
        let (title_tx, title_rx) = broadcast::channel::<ozmux_multiplexer::WindowId>(8);
        let wid = ozmux_multiplexer::WindowId::new();
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(super::run_bridge_task(
            vt_state.clone(),
            pty_rx,
            reply_rx,
            control_rx,
            Some(wid.clone()),
            title_tx,
            cancel.clone(),
            BridgeConfig::default(),
            ActivityId::new(),
        ));
        TitleBridge {
            control_tx,
            title_rx,
            vt_state,
            wid,
            cancel,
            handle,
            _pty_tx: pty_tx,
        }
    }

    #[tokio::test]
    async fn bridge_task_captures_and_sanitizes_title() {
        let mut bridge = spawn_title_bridge();

        bridge
            .control_tx
            .send(ControlFrame::Title("hi\x07there".into()))
            .await
            .unwrap();
        let signalled = bridge.title_rx.recv().await.unwrap();
        assert_eq!(signalled, bridge.wid);
        assert_eq!(
            bridge.vt_state.lock().unwrap().title.as_deref(),
            Some("hithere"),
            "control char should be stripped before storage"
        );

        bridge.cancel.cancel();
        let _ = bridge.handle.await;
    }

    #[tokio::test]
    async fn bridge_task_reset_title_clears_stored_title() {
        let mut bridge = spawn_title_bridge();

        bridge
            .control_tx
            .send(ControlFrame::Title("seed".into()))
            .await
            .unwrap();
        let _ = bridge.title_rx.recv().await.unwrap();
        assert_eq!(
            bridge.vt_state.lock().unwrap().title.as_deref(),
            Some("seed"),
            "initial title should be set"
        );

        bridge
            .control_tx
            .send(ControlFrame::ResetTitle)
            .await
            .unwrap();
        let _ = bridge.title_rx.recv().await.unwrap();
        assert_eq!(
            bridge.vt_state.lock().unwrap().title,
            None,
            "title should be cleared after ResetTitle"
        );

        bridge.cancel.cancel();
        let _ = bridge.handle.await;
    }

    #[tokio::test]
    async fn bridge_task_suppresses_unchanged_title() {
        let mut bridge = spawn_title_bridge();

        bridge
            .control_tx
            .send(ControlFrame::Title("same".into()))
            .await
            .unwrap();
        assert_eq!(bridge.title_rx.recv().await.unwrap(), bridge.wid);

        bridge
            .control_tx
            .send(ControlFrame::Title("same".into()))
            .await
            .unwrap();
        bridge
            .control_tx
            .send(ControlFrame::Title("different".into()))
            .await
            .unwrap();
        assert_eq!(
            bridge.title_rx.recv().await.unwrap(),
            bridge.wid,
            "only the changed title should be signalled; the repeat is dropped"
        );
        assert!(
            bridge.title_rx.try_recv().is_err(),
            "no extra signal for the unchanged title"
        );

        bridge.cancel.cancel();
        let _ = bridge.handle.await;
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

    #[test]
    fn decide_frame_kind_below_threshold_keeps_partial_at_84_percent() {
        // 21/25 rows dirty (84%) — must remain Delta (84 < 85).
        let mut state = make_state(80, 25);
        state.first_emit = false;
        let rows: Vec<u16> = (0..21).collect();
        let kind = decide_frame_kind(&state, DirtyRows::Rows(rows));
        assert!(matches!(kind, FrameKind::Delta { .. }));
    }

    #[test]
    fn decide_frame_kind_at_threshold_promotes_to_full_at_85_percent() {
        // 17/20 rows dirty (exactly 85%) — must promote to Snapshot.
        let mut state = make_state(80, 20);
        state.first_emit = false;
        let rows: Vec<u16> = (0..17).collect();
        let kind = decide_frame_kind(&state, DirtyRows::Rows(rows));
        assert!(matches!(kind, FrameKind::Snapshot { .. }));
    }

    #[test]
    fn decide_frame_kind_full_damage_always_snapshot() {
        let mut state = make_state(80, 24);
        state.first_emit = false;
        let kind = decide_frame_kind(&state, DirtyRows::Full);
        assert!(matches!(kind, FrameKind::Snapshot { .. }));
    }

    #[test]
    fn oversize_error_json_has_expected_fields() {
        let seq: u32 = 42;
        let json = serde_json::json!({
            "kind": "error",
            "seq": seq,
            "category": "frame_size_exceeded",
        })
        .to_string();
        assert!(json.contains("\"kind\":\"error\""));
        assert!(json.contains("\"category\":\"frame_size_exceeded\""));
        assert!(json.contains("\"seq\":42"));
    }
}
