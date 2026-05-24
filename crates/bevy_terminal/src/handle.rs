//! `TerminalHandle` — Component holding alacritty `Term` + bridge state.

use crate::coalescer::Coalescer;
use crate::events::{
    TerminalBell, TerminalClipboardStore, TerminalModeChanged, TerminalTitleChanged,
};
use crate::pty::PtyHandle;
use crate::title::{TerminalTitle, sanitize_title};
use crate::vt::damage::{DamageVerdict, DirtyRows};
use crate::vt::frame_builder::{build_delta, build_snapshot, extract_cursor, viewport_row_to_line};
use crate::vt::hyperlink::HyperlinkInterner;
use crate::vt::listener::{ControlFrame, TermListener};
use crate::vt::mode_diff::diff_mode;
use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::ansi::{Color as AColor, Rgb};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::system::Commands;
use bevy_terminal_renderer::prelude::{Cursor, CursorShape, SnapshotReason};
use crossbeam_channel::Receiver;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Inner `Dimensions` impl exposed to `Term::new` / `Term::resize`.
///
/// Alacritty's own `TermSize` is `pub(crate)` so we provide a minimal
/// local equivalent.
pub(crate) struct LocalDim {
    cols: usize,
    rows: usize,
}

impl LocalDim {
    pub(crate) fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.into(),
            rows: rows.into(),
        }
    }
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

/// All VT / bridge state for a single terminal entity.
#[derive(Component)]
pub struct TerminalHandle {
    term: Term<TermListener>,
    parser: Processor,
    hyperlinks: HyperlinkInterner,
    prev_cursor: Option<Cursor>,
    pending_user_input: bool,
    pending_damage: Option<DirtyRows>,
    first_emit: bool,
    scratch_dirty: Vec<u16>,
    row_hashes: HashMap<i32, u64>,
    window_open_mode: Option<TermMode>,
    frame_seq: u32,
    reply_rx: Receiver<Vec<u8>>,
    control_rx: Receiver<ControlFrame>,
}

impl TerminalHandle {
    /// Constructs a fresh handle from the matched dims + channel set.
    /// Called only from `TerminalBundle::spawn`.
    pub(crate) fn new(
        cols: u16,
        rows: u16,
        listener: TermListener,
        reply_rx: Receiver<Vec<u8>>,
        control_rx: Receiver<ControlFrame>,
    ) -> Self {
        let size = LocalDim::new(cols, rows);
        let term = Term::new(Config::default(), &size, listener);
        Self {
            term,
            parser: Processor::new(),
            hyperlinks: HyperlinkInterner::new(),
            prev_cursor: None,
            pending_user_input: false,
            pending_damage: None,
            first_emit: true,
            scratch_dirty: Vec::new(),
            row_hashes: HashMap::new(),
            window_open_mode: None,
            frame_seq: 0,
            reply_rx,
            control_rx,
        }
    }

    /// Feeds a chunk of PTY bytes through the vte parser into `Term`.
    pub fn advance(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.parser.advance(&mut self.term, chunk);
    }

    /// Returns true if the current `Term` cursor differs from the most
    /// recently emitted cursor (`prev_cursor`).
    pub fn cursor_changed(&self) -> bool {
        let curr = extract_cursor(&self.term);
        self.prev_cursor.as_ref().is_none_or(|prev| *prev != curr)
    }

    /// Writes bytes to the PTY master.
    ///
    /// # Invariants
    ///
    /// `pending_user_input` is set to `true` BEFORE the PTY write so a
    /// racing emit cycle that observes the user input cannot miss the
    /// flag. The coalescer's `AtMostOneRow + pending_user_input`
    /// immediate-flush rule depends on this ordering — without it,
    /// keyboard echo degrades to the IDLE deadline (≈1 Bevy frame).
    pub fn write(&mut self, pty: &mut PtyHandle, bytes: &[u8]) -> std::io::Result<()> {
        self.pending_user_input = true;
        pty.write_all(bytes)
    }

    /// Resizes the alacritty grid and the PTY master together.
    ///
    /// # Invariants
    ///
    /// - `row_hashes` is cleared right after `term.resize(dim)` so
    ///   post-resize rows are not hash-filtered against pre-resize
    ///   hashes.
    /// - The resulting Full damage is staged on `pending_damage` and
    ///   the coalescer is armed with `Instant::now()`. Without this,
    ///   resizing an idle terminal (no PTY output pending) would
    ///   never reach the renderer until the next genuine chunk —
    ///   `check_deadline_flush` only fires when the coalescer is
    ///   armed or the bootstrap rescue triggers.
    pub fn resize(
        &mut self,
        pty: &mut PtyHandle,
        coalescer: &mut Coalescer,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<()> {
        let dim = LocalDim::new(cols, rows);
        self.term.resize(dim);
        self.row_hashes.clear();
        pty.resize(cols, rows)?;
        self.stage_full_damage_and_arm(coalescer);
        Ok(())
    }

    /// Scrolls the visible viewport by `delta` lines and arms an
    /// emit. Positive `delta` moves backward into scrollback history;
    /// negative moves forward toward the live tail. Alacritty clamps
    /// to `[0, history_size]`.
    ///
    /// # Invariants
    ///
    /// The Full damage produced by `Term::scroll_display` is staged
    /// and the coalescer is armed so the new viewport reaches the
    /// renderer even when the shell is idle.
    pub fn scroll(&mut self, coalescer: &mut Coalescer, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
        self.stage_full_damage_and_arm(coalescer);
    }

    /// Snaps the viewport to the live tail and arms an emit.
    pub fn scroll_to_bottom(&mut self, coalescer: &mut Coalescer) {
        self.term.scroll_display(Scroll::Bottom);
        self.stage_full_damage_and_arm(coalescer);
    }

    /// Captures the current `Term::damage()` state into
    /// `pending_damage` and arms the coalescer for an immediate
    /// deadline-driven emit. Used by `resize` / `scroll` /
    /// `scroll_to_bottom` to wake the bridge when no PTY chunks are
    /// in flight.
    fn stage_full_damage_and_arm(&mut self, coalescer: &mut Coalescer) {
        let mut scratch = std::mem::take(&mut self.scratch_dirty);
        self.pending_damage = Some(DirtyRows::collect(&mut self.term, &mut scratch));
        self.scratch_dirty = scratch;
        coalescer.arm_or_extend(std::time::Instant::now());
    }

    /// Reads the current cols / rows / cursor.
    pub fn read_geometry(&self) -> (u16, u16, Cursor) {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        let cursor = extract_cursor(&self.term);
        (cols, rows, cursor)
    }

    /// Advance with a PTY chunk: capture pre-advance mode (for window
    /// arming), advance Term, collect damage, classify the verdict,
    /// and decide whether to flush immediately. Returns `true` when
    /// the caller should call `emit`; `false` when it should arm
    /// the coalescer.
    ///
    /// # Invariants
    ///
    /// - `window_open_mode` is set to `pre_advance_mode` if (a) the
    ///   coalescer is not armed and (b) no window mode is already
    ///   captured.
    /// - On `AtMostOneRow` immediate-flush, `pending_user_input` is
    ///   cleared to prevent double-flushing on the next chunk.
    pub(crate) fn ingest_chunk(&mut self, chunk: &[u8], coalescer: &Coalescer) -> bool {
        let pre_advance_mode = *self.term.mode();
        self.advance(chunk);
        if !coalescer.is_armed() && self.window_open_mode.is_none() {
            self.window_open_mode = Some(pre_advance_mode);
        }
        let mut scratch = std::mem::take(&mut self.scratch_dirty);
        let dirty = DirtyRows::collect(&mut self.term, &mut scratch);
        self.scratch_dirty = scratch;
        let verdict = DamageVerdict::classify_damage(&dirty, self.cursor_changed());
        let flush =
            coalescer.should_flush_immediately(self.first_emit, &verdict, self.pending_user_input);
        if flush && self.pending_user_input && matches!(verdict, DamageVerdict::AtMostOneRow) {
            self.pending_user_input = false;
        }
        self.pending_damage = Some(dirty);
        flush
    }

    /// True iff this handle has never emitted a frame AND no pending
    /// damage is staged. Used by `check_deadline_flush` to detect that
    /// a freshly spawned terminal needs the bootstrap snapshot rescue.
    pub(crate) fn needs_bootstrap_emit(&self) -> bool {
        self.first_emit && self.pending_damage.is_none()
    }

    /// Forces a one-shot `Term::damage()` collection to populate
    /// `pending_damage`. Called only by the bootstrap rescue path when
    /// `needs_bootstrap_emit()` is true.
    pub(crate) fn force_bootstrap_damage(&mut self) {
        let mut scratch = std::mem::take(&mut self.scratch_dirty);
        self.pending_damage = Some(DirtyRows::collect(&mut self.term, &mut scratch));
        self.scratch_dirty = scratch;
    }

    /// Drains alacritty control events (Bell / Title / ResetTitle /
    /// Clipboard) and emits the corresponding `EntityEvent`s on this
    /// `entity`. Updates the supplied `TerminalTitle` Component as a
    /// side-effect of `Title` / `ResetTitle`.
    pub(crate) fn drain_control_events(
        &self,
        commands: &mut Commands,
        entity: Entity,
        title: &mut TerminalTitle,
    ) {
        while let Ok(ctrl) = self.control_rx.try_recv() {
            match ctrl {
                ControlFrame::Bell => {
                    commands.trigger(TerminalBell { entity });
                }
                ControlFrame::Title(s) => {
                    let sanitized = sanitize_title(&s);
                    title.0 = Some(sanitized.clone());
                    commands.trigger(TerminalTitleChanged {
                        entity,
                        title: Some(sanitized),
                    });
                }
                ControlFrame::ResetTitle => {
                    title.0 = None;
                    commands.trigger(TerminalTitleChanged {
                        entity,
                        title: None,
                    });
                }
                ControlFrame::Clipboard { content, .. } => {
                    commands.trigger(TerminalClipboardStore { entity, content });
                }
            }
        }
    }

    /// Drains `reply_rx` (alacritty `PtyWrite` reply bytes) into the
    /// supplied buffer. Caller decides when to actually `write_all`
    /// the concatenated bytes to the PTY.
    pub(crate) fn drain_replies_into(&self, buf: &mut Vec<u8>) {
        while let Ok(bytes) = self.reply_rx.try_recv() {
            buf.extend_from_slice(&bytes);
        }
    }

    /// Emit a frame for the damage stashed on `self.pending_damage`.
    /// Disarms the coalescer.
    pub(crate) fn emit(
        &mut self,
        commands: &mut Commands,
        entity: Entity,
        coalescer: &mut Coalescer,
    ) {
        let Some(mut dirty) = self.pending_damage.take() else {
            self.abort_emit_with_no_damage(coalescer);
            return;
        };

        let prev_mode = self.consume_window_open_mode();
        let curr_mode = *self.term.mode();
        let curr_cursor = extract_cursor(&self.term);
        let kept_hashes = self.filter_unchanged_dirty_rows(&mut dirty, &curr_cursor);

        if self.is_noop_emit(&dirty, &curr_cursor, prev_mode, curr_mode) {
            self.finalize_emit(coalescer);
            return;
        }

        let kind = decide_frame_kind(self, dirty);
        self.first_emit = false;
        let seq = self.next_frame_seq();

        self.announce_mode_change(commands, entity, prev_mode, curr_mode);
        match kind {
            FrameKind::Snapshot { reason } => self.emit_snapshot(commands, entity, seq, reason),
            FrameKind::Delta { rows } => self.emit_delta(commands, entity, seq, rows, kept_hashes),
        }

        self.prev_cursor = Some(curr_cursor);
        self.finalize_emit(coalescer);
    }

    /// Cleanup path taken when `emit` is invoked with no staged
    /// damage. Disarms the coalescer and discards any captured
    /// window-open mode so the next chunk re-captures fresh.
    fn abort_emit_with_no_damage(&mut self, coalescer: &mut Coalescer) {
        coalescer.disarm();
        self.window_open_mode = None;
    }

    /// Takes the captured `window_open_mode` if present, falling back
    /// to the current `Term::mode()` snapshot. The caller compares
    /// against `*self.term.mode()` to compute the wire mode diff.
    fn consume_window_open_mode(&mut self) -> TermMode {
        self.window_open_mode
            .take()
            .unwrap_or_else(|| *self.term.mode())
    }

    /// Skips rows whose content hash matches the previously emitted
    /// value and returns the `(line_index, fresh_hash)` pairs the
    /// caller should record after a Delta emit.
    ///
    /// # Invariants
    ///
    /// Called BEFORE [`decide_frame_kind`]. Otherwise unchanged rows
    /// that alacritty implicitly re-damages (e.g. the cursor row)
    /// inflate the row count past the 85 % threshold and false-promote
    /// a Delta into a full Snapshot.
    fn filter_unchanged_dirty_rows(
        &self,
        dirty: &mut DirtyRows,
        curr_cursor: &Cursor,
    ) -> Vec<(i32, u64)> {
        let DirtyRows::Rows(rows) = dirty else {
            return Vec::new();
        };
        let term = &self.term;
        let row_hashes = &self.row_hashes;
        let mut kept: Vec<(i32, u64)> = Vec::new();
        rows.retain(|&viewport_y| {
            let line = viewport_row_to_line(term, viewport_y as i32);
            let h = hash_row(term, line, curr_cursor, viewport_y);
            let stale = row_hashes.get(&line.0).copied();
            if Some(h) == stale {
                false
            } else {
                kept.push((line.0, h));
                true
            }
        });
        kept
    }

    /// True when there is nothing observable to broadcast: no dirty
    /// rows remain after filtering, the mode is unchanged, the cursor
    /// is unchanged, AND this is not the bootstrap emit.
    fn is_noop_emit(
        &self,
        dirty: &DirtyRows,
        curr_cursor: &Cursor,
        prev_mode: TermMode,
        curr_mode: TermMode,
    ) -> bool {
        let dirty_is_empty = matches!(dirty, DirtyRows::Rows(r) if r.is_empty());
        let cursor_unchanged = self
            .prev_cursor
            .as_ref()
            .is_some_and(|prev| *prev == *curr_cursor);
        dirty_is_empty && prev_mode == curr_mode && cursor_unchanged && !self.first_emit
    }

    /// Returns the current `frame_seq`, advancing it via
    /// `wrapping_add(1)` for the next emit. The renderer's
    /// `material::state` rebuild trigger compares `grid.last_seq !=
    /// state.last_grid_seq`, so a stuck seq would freeze rendering.
    fn next_frame_seq(&mut self) -> u32 {
        let seq = self.frame_seq;
        self.frame_seq = self.frame_seq.wrapping_add(1);
        seq
    }

    /// Emits a `TerminalModeChanged` trigger when tracked `TermMode`
    /// bits differ between the coalescer-window-open snapshot and the
    /// current state.
    ///
    /// # Invariants
    ///
    /// Called BEFORE the frame trigger so subscribers apply
    /// mode-related side-effects (e.g. alt-screen swap, mouse
    /// reporting) before re-rendering.
    fn announce_mode_change(
        &self,
        commands: &mut Commands,
        entity: Entity,
        prev_mode: TermMode,
        curr_mode: TermMode,
    ) {
        let mode_change = diff_mode(prev_mode, curr_mode);
        if mode_change.is_empty() {
            return;
        }
        let added: Vec<String> = mode_change.added.into_iter().map(String::from).collect();
        let removed: Vec<String> = mode_change.removed.into_iter().map(String::from).collect();
        commands.trigger(TerminalModeChanged {
            entity,
            added,
            removed,
        });
    }

    /// Builds + triggers a `FrameSnapshot`, then rebuilds
    /// `row_hashes` from scratch so subsequent Delta emits can
    /// hash-filter against the snapshot baseline.
    fn emit_snapshot(
        &mut self,
        commands: &mut Commands,
        entity: Entity,
        seq: u32,
        reason: SnapshotReason,
    ) {
        let snap = build_snapshot(&self.term, entity, seq, reason, &mut self.hyperlinks);
        commands.trigger(snap);
        self.rebuild_full_row_hashes();
    }

    /// Rebuilds `row_hashes` for every visible viewport row using the
    /// current cursor overlay. Called only from the Snapshot path of
    /// `emit`.
    fn rebuild_full_row_hashes(&mut self) {
        self.row_hashes.clear();
        let screen_rows = self.term.screen_lines() as u16;
        let snap_cursor = extract_cursor(&self.term);
        for viewport_y in 0..screen_rows {
            let line = viewport_row_to_line(&self.term, viewport_y as i32);
            let h = hash_row(&self.term, line, &snap_cursor, viewport_y);
            self.row_hashes.insert(line.0, h);
        }
    }

    /// Builds + triggers a `FrameDelta`, restores the consumed
    /// `scratch_dirty` Vec for next-cycle reuse, and folds
    /// `kept_hashes` into `row_hashes` so subsequent Delta emits can
    /// hash-filter correctly.
    fn emit_delta(
        &mut self,
        commands: &mut Commands,
        entity: Entity,
        seq: u32,
        rows: Vec<u16>,
        kept_hashes: Vec<(i32, u64)>,
    ) {
        let delta = build_delta(&self.term, entity, seq, &rows, &mut self.hyperlinks);
        self.scratch_dirty = rows;
        commands.trigger(delta);
        for (line_i32, h) in kept_hashes {
            self.row_hashes.insert(line_i32, h);
        }
    }

    /// Resets alacritty's per-cycle damage tracker and disarms the
    /// coalescer. Called at the end of every emit (both the noop-skip
    /// and the normal-end path).
    fn finalize_emit(&mut self, coalescer: &mut Coalescer) {
        self.term.reset_damage();
        coalescer.disarm();
    }
}

/// Classification used by `decide_frame_kind` to select snapshot vs
/// delta. Local to this module — `frame_builder` doesn't need it.
enum FrameKind {
    Snapshot { reason: SnapshotReason },
    Delta { rows: Vec<u16> },
}

/// Row-count fraction at which Partial damage promotes to a Snapshot.
/// `partial * SNAPSHOT_THRESHOLD_NUM >= total * SNAPSHOT_THRESHOLD_DEN`
/// holds when partial / total >= 17 / 20 = 85 %. Beyond this fraction,
/// a full snapshot is more bandwidth-efficient than enumerating dirty
/// rows.
const SNAPSHOT_THRESHOLD_NUM: u32 = 20;
const SNAPSHOT_THRESHOLD_DEN: u32 = 17;

/// Selects the frame type. Policy (priority order):
/// 1. `state.first_emit` → `Snapshot { reason: Initial }`
/// 2. `DirtyRows::Full` → `Snapshot { reason: Resize }`
/// 3. Partial damage >= 85 % of total rows → `Snapshot { reason: Resize }`
/// 4. Otherwise → `Delta { rows }`
fn decide_frame_kind(handle: &TerminalHandle, dirty: DirtyRows) -> FrameKind {
    let total_rows = handle.term.screen_lines() as u16;
    if handle.first_emit {
        return FrameKind::Snapshot {
            reason: SnapshotReason::Initial,
        };
    }
    match dirty {
        DirtyRows::Full => FrameKind::Snapshot {
            reason: SnapshotReason::Resize,
        },
        DirtyRows::Rows(rows) => {
            if (rows.len() as u32) * SNAPSHOT_THRESHOLD_NUM
                >= (total_rows as u32) * SNAPSHOT_THRESHOLD_DEN
            {
                FrameKind::Snapshot {
                    reason: SnapshotReason::Resize,
                }
            } else {
                FrameKind::Delta { rows }
            }
        }
    }
}

/// Computes a content hash for a single grid row, including the
/// cursor overlay when the cursor lands on this `viewport_y`.
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

fn hash_acolor(c: &AColor, h: &mut DefaultHasher) {
    match c {
        AColor::Named(n) => {
            0u8.hash(h);
            (*n as u32).hash(h);
        }
        AColor::Spec(Rgb { r, g, b }) => {
            1u8.hash(h);
            r.hash(h);
            g.hash(h);
            b.hash(h);
        }
        AColor::Indexed(i) => {
            2u8.hash(h);
            i.hash(h);
        }
    }
}

fn hash_cursor_shape(s: CursorShape, h: &mut DefaultHasher) {
    let n: u8 = match s {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
    };
    n.hash(h);
}
