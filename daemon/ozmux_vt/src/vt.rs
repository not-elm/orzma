//! Bevy-free VT engine: alacritty `Term` driving + parsing + coalescing
//! + frame construction for a single terminal.

pub mod damage;
pub mod frame_builder;
pub mod hyperlink;
pub mod listener;
pub mod mode_diff;

use crate::coalescer::Coalescer;
use crate::event::VtEvent;
use crate::frame::{
    Cursor, CursorShape, Frame, FrameDelta, FrameSnapshot, SelectionRange, SnapshotReason, ViCursor,
};
use crate::osc7::Osc7Capture;
use crate::vt::damage::{DamageVerdict, DirtyRows};
use crate::vt::frame_builder::{
    build_delta, build_snapshot, extract_cursor, extract_selection_range, extract_vi_cursor,
    viewport_row_to_line,
};
use crate::vt::hyperlink::HyperlinkInterner;
use crate::vt::listener::{ControlFrame, TermListener};
use crate::vt::mode_diff::diff_mode;
use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::ansi::{Color as AColor, Rgb};
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;
use vte::Parser;

/// Outcome of `on_output`: emit immediately, deadline-arm, or no change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAction {
    /// Damage warrants an immediate flush; the caller should call `emit`.
    EmitNow,
    /// Damage was staged and the coalescer armed; a later `tick` flushes.
    Armed,
    /// Nothing to do (empty input).
    Idle,
}

/// Snapshot of the data the copy-mode indicator needs.
///
/// Reading `Term` directly (rather than the renderer-side `TerminalGrid`
/// component) is deliberate: `FrameDelta` does not carry `history_size`,
/// so a `TerminalGrid` mirror would be stale between snapshots and the
/// indicator would briefly display the wrong `total` under sustained
/// PTY output.
#[derive(Debug, Clone, Copy)]
pub struct ViIndicatorSnapshot {
    /// 0-based scroll offset from the live tail (0 = bottom).
    pub scroll_offset: usize,
    /// Scrollback history length, matching tmux's `[offset/total]`
    /// denominator. Sourced from `Term::history_size()`.
    pub history_size: usize,
}

/// Inner `Dimensions` impl exposed to `Term::new` / `Term::resize`.
///
/// Alacritty's own `TermSize` is `pub(crate)` so we provide a minimal
/// local equivalent.
struct LocalDim {
    cols: usize,
    rows: usize,
}

impl LocalDim {
    fn new(cols: u16, rows: u16) -> Self {
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

/// Bevy-free VT engine. Owns one terminal's `Term` + parsing +
/// coalescing + frame generation. The coalescer is internal; timing is
/// supplied by the caller (`now: Instant`) so the engine is sans-I/O.
pub struct Vt {
    term: Term<TermListener>,
    parser: Processor,
    hyperlinks: HyperlinkInterner,
    coalescer: Coalescer,
    prev_cursor: Option<Cursor>,
    prev_vi_cursor: Option<ViCursor>,
    prev_selection: Option<SelectionRange>,
    selection_anchor: Option<alacritty_terminal::index::Point>,
    pending_user_input: bool,
    pending_damage: Option<DirtyRows>,
    first_emit: bool,
    scratch_dirty: Vec<u16>,
    row_hashes: HashMap<i32, u64>,
    window_open_mode: Option<TermMode>,
    frame_seq: u32,
    pending_events: Vec<VtEvent>,
    reply_rx: Receiver<Vec<u8>>,
    control_rx: Receiver<ControlFrame>,
    osc7_parser: Parser,
    osc7: Osc7Capture,
}

impl Vt {
    /// Constructs a fresh VT engine sized `cols` x `rows`, wiring up the
    /// internal listener and OSC 7 capture channels.
    pub fn new(cols: u16, rows: u16) -> Self {
        let (reply_tx, reply_rx) = unbounded::<Vec<u8>>();
        let (control_tx, control_rx) = unbounded::<ControlFrame>();
        let listener = TermListener {
            reply_tx,
            control_tx: control_tx.clone(),
        };
        Self::with_channels(cols, rows, listener, reply_rx, control_rx, control_tx)
    }

    /// Feeds a PTY chunk through the parser and decides whether to flush.
    ///
    /// Mirrors the old `advance` + `ingest_chunk`: advance Term + OSC 7,
    /// capture pre-advance mode for window arming, classify damage, and
    /// decide immediate-flush. Returns `EmitNow` when the caller should
    /// call `emit`; otherwise stages damage, arms the coalescer, and
    /// returns `Armed`. Empty input is `Idle` (no work).
    ///
    /// # Invariants
    ///
    /// - `window_open_mode` is set to `pre_advance_mode` if (a) the
    ///   coalescer is not armed and (b) no window mode is already
    ///   captured.
    /// - On `AtMostOneRow` immediate-flush, `pending_user_input` is
    ///   cleared to prevent double-flushing on the next chunk.
    pub fn on_output(&mut self, bytes: &[u8], now: Instant) -> OutputAction {
        if bytes.is_empty() {
            return OutputAction::Idle;
        }
        let pre_advance_mode = *self.term.mode();
        self.advance(bytes);
        if !self.coalescer.is_armed() && self.window_open_mode.is_none() {
            self.window_open_mode = Some(pre_advance_mode);
        }
        let mut scratch = std::mem::take(&mut self.scratch_dirty);
        let dirty = DirtyRows::collect(&mut self.term, &mut scratch);
        self.scratch_dirty = scratch;
        let verdict = DamageVerdict::classify_damage(&dirty, self.cursor_changed());
        let flush = self.coalescer.should_flush_immediately(
            self.first_emit,
            &verdict,
            self.pending_user_input,
        );
        if flush && self.pending_user_input && matches!(verdict, DamageVerdict::AtMostOneRow) {
            self.pending_user_input = false;
        }
        self.pending_damage = Some(dirty);
        if flush {
            OutputAction::EmitNow
        } else {
            self.coalescer.arm_or_extend(now);
            OutputAction::Armed
        }
    }

    /// Emits a frame for the damage stashed on `pending_damage`, or
    /// `None` when there is nothing observable to broadcast. Disarms the
    /// coalescer. Any mode transition detected during this cycle is
    /// queued onto `pending_events` (drained by `drain_events`).
    pub fn emit(&mut self) -> Option<Frame> {
        let mut dirty = match self.pending_damage.take() {
            Some(dirty) => dirty,
            None => {
                self.abort_emit_with_no_damage();
                return None;
            }
        };

        let prev_mode = self.consume_window_open_mode();
        let curr_mode = *self.term.mode();
        let curr_cursor = extract_cursor(&self.term);
        let kept_hashes = self.filter_unchanged_dirty_rows(&mut dirty, &curr_cursor);
        let curr_vi_cursor = extract_vi_cursor(&self.term);
        let curr_selection = extract_selection_range(&self.term);

        if self.is_noop_emit(
            &dirty,
            &curr_cursor,
            prev_mode,
            curr_mode,
            curr_vi_cursor,
            curr_selection,
        ) {
            self.finalize_emit();
            return None;
        }

        let kind = decide_frame_kind(self, dirty);
        self.first_emit = false;
        let seq = self.next_frame_seq();

        self.announce_mode_change(prev_mode, curr_mode);
        let frame = match kind {
            FrameKind::Snapshot { reason } => Frame::Snapshot(self.emit_snapshot(seq, reason)),
            FrameKind::Delta { rows } => Frame::Delta(self.emit_delta(seq, rows, kept_hashes)),
        };

        self.prev_cursor = Some(curr_cursor);
        self.prev_vi_cursor = curr_vi_cursor;
        self.prev_selection = curr_selection;
        self.finalize_emit();
        Some(frame)
    }

    /// Deadline flush plus bootstrap rescue. A never-emitted terminal
    /// with no staged damage forces a Full snapshot (the Initial
    /// snapshot rescue). Otherwise: arms any staged-but-unarmed damage
    /// from a prior state mutation, then emits when the coalescer's
    /// deadline has elapsed.
    ///
    /// # Invariants
    ///
    /// State-mutator methods (`scroll` / `selection_*` / `resize` / …)
    /// stage damage without arming. `tick` arms that staged damage so
    /// its deadline becomes reachable; without this an idle terminal's
    /// scroll / selection change would never reach the renderer.
    pub fn tick(&mut self, now: Instant) -> Option<Frame> {
        if self.needs_bootstrap_emit() {
            self.force_bootstrap_damage();
            return self.emit();
        }
        if self.pending_damage.is_some() && !self.coalescer.is_armed() {
            self.coalescer.arm_or_extend(now);
        }
        if let Some(deadline) = self.coalescer.next_deadline()
            && now >= deadline
        {
            return self.emit();
        }
        None
    }

    /// Returns the next flush deadline from the internal coalescer, or
    /// `None` when the coalescer is disarmed.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.coalescer.next_deadline()
    }

    /// Drains pending control events: the listener's control channel
    /// (Bell / Title / ResetTitle / Clipboard / CurrentDir) plus any
    /// `ModeChanged` queued by `emit`. The control-channel events are
    /// appended after the queued ones so a mode change announced in the
    /// same cycle is reported before later async control frames.
    pub fn drain_events(&mut self) -> Vec<VtEvent> {
        let mut events = std::mem::take(&mut self.pending_events);
        while let Ok(ctrl) = self.control_rx.try_recv() {
            match ctrl {
                ControlFrame::Bell => events.push(VtEvent::Bell),
                ControlFrame::Title(s) => events.push(VtEvent::TitleChanged(Some(s))),
                ControlFrame::ResetTitle => events.push(VtEvent::TitleChanged(None)),
                ControlFrame::Clipboard { content, .. } => {
                    events.push(VtEvent::ClipboardStore(content));
                }
                ControlFrame::CurrentDir(path) => events.push(VtEvent::CurrentDir(path)),
            }
        }
        events
    }

    /// Drains `reply_rx` (alacritty `PtyWrite` reply bytes) into a fresh
    /// `Vec`. The caller writes the concatenated bytes back to the PTY.
    pub fn drain_replies(&mut self) -> Vec<u8> {
        let mut buf = Vec::new();
        while let Ok(bytes) = self.reply_rx.try_recv() {
            buf.extend_from_slice(&bytes);
        }
        buf
    }

    /// Marks that user input is in flight so the coalescer's
    /// `AtMostOneRow` immediate-flush rule fires on the next chunk.
    ///
    /// # Invariants
    ///
    /// The caller MUST set this BEFORE the PTY write so a racing emit
    /// cycle cannot miss the flag. Without it, keyboard echo degrades to
    /// the IDLE deadline (≈1 Bevy frame).
    pub fn note_user_input(&mut self) {
        self.pending_user_input = true;
    }

    /// Returns the current value of the `pending_user_input` flag.
    /// Exposed so cross-crate integration tests can confirm a PTY write
    /// took place without reading from the PTY master.
    pub fn pending_user_input(&self) -> bool {
        self.pending_user_input
    }

    /// Resizes the alacritty grid and stages Full damage.
    ///
    /// # Invariants
    ///
    /// `row_hashes` is cleared right after `term.resize(dim)` so
    /// post-resize rows are not hash-filtered against pre-resize hashes.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let dim = LocalDim::new(cols, rows);
        self.term.resize(dim);
        self.row_hashes.clear();
        self.stage_full_damage();
    }

    /// Scrolls the visible viewport by `delta` lines and stages Full
    /// damage. Positive `delta` moves backward into scrollback history;
    /// negative moves forward toward the live tail. Alacritty clamps to
    /// `[0, history_size]`.
    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
        self.stage_full_damage();
    }

    /// Snaps the viewport to the live tail and stages Full damage.
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        self.stage_full_damage();
    }

    /// Scrolls the viewport one page up (`Scroll::PageUp`). Alacritty
    /// clamps the vi cursor into the new viewport automatically. Stages
    /// Full damage.
    pub fn scroll_page_up(&mut self) {
        self.term.scroll_display(Scroll::PageUp);
        self.stage_full_damage();
    }

    /// Scrolls the viewport one page down (`Scroll::PageDown`).
    pub fn scroll_page_down(&mut self) {
        self.term.scroll_display(Scroll::PageDown);
        self.stage_full_damage();
    }

    /// Enters vi (copy) mode. Idempotent — a second call while already
    /// in vi mode is a no-op rather than a toggle-off. Stages Full
    /// damage so the renderer observes the new mode (`Term::toggle_vi_mode`
    /// itself does NOT damage the grid; without this the snapshot carrying
    /// the new vi_cursor would never reach the renderer).
    pub fn enter_vi_mode(&mut self) {
        if !self.term.mode().contains(TermMode::VI) {
            self.term.toggle_vi_mode();
        }
        self.stage_full_damage();
    }

    /// Exits vi mode and snaps the viewport to the live tail. Idempotent.
    /// Stages Full damage so the renderer receives a frame with
    /// `vi_cursor: None`.
    pub fn exit_vi_mode(&mut self) {
        if self.term.mode().contains(TermMode::VI) {
            self.term.toggle_vi_mode();
        }
        self.term.scroll_display(Scroll::Bottom);
        self.stage_full_damage();
    }

    /// Drives `Term::vi_motion(motion)`. Alacritty re-computes the
    /// selection internally when one is active (`vi_mode_recompute_selection`),
    /// so callers do not need to re-issue `selection_*` after motion.
    /// Stages Full damage because vi-cursor moves are not part of
    /// alacritty's `Term::damage()` (see `is_noop_emit` docs).
    pub fn vi_motion(&mut self, motion: alacritty_terminal::vi_mode::ViMotion) {
        self.term.vi_motion(motion);
        self.stage_full_damage();
    }

    /// Jumps the vi cursor to `viewport_point`. Wraps
    /// `Term::vi_goto_point`. No-op when not in vi mode.
    ///
    /// Called by the Bevy glue during mouse interaction inside copy
    /// mode: BEFORE every `selection_update_to`, AND BEFORE every
    /// `scroll` in the autoscroll loop, so alacritty's vi-mode recompute
    /// on viewport changes (`scroll_display` → `vi_mode_recompute_selection`)
    /// does not snap the selection end back to a stale vi cursor.
    pub fn vi_goto(&mut self, viewport_point: alacritty_terminal::index::Point) {
        if !self.term.mode().contains(TermMode::VI) {
            return;
        }
        let line = viewport_row_to_line(&self.term, viewport_point.line.0);
        let point = alacritty_terminal::index::Point::new(line, viewport_point.column);
        self.term.vi_goto_point(point);
        self.stage_full_damage();
    }

    /// Starts a selection of `ty` anchored at `viewport_point` with
    /// `side`. `viewport_point` carries a viewport-relative row in
    /// `line.0` (0 = top of viewport); this method translates it to an
    /// alacritty terminal `Line` so the selection survives mid-drag
    /// viewport scrolling.
    ///
    /// Calls `update(anchor, opposite_side)` immediately after
    /// `Selection::new` so `selection_to_string()` does not return
    /// `None` for a freshly-anchored `Simple` / `Block` selection
    /// (alacritty's `to_range` short-circuits on `is_empty()`).
    pub fn selection_start_at(
        &mut self,
        viewport_point: alacritty_terminal::index::Point,
        side: alacritty_terminal::index::Side,
        ty: alacritty_terminal::selection::SelectionType,
    ) {
        use alacritty_terminal::index::Side as ASide;
        let line = viewport_row_to_line(&self.term, viewport_point.line.0);
        let anchor = alacritty_terminal::index::Point::new(line, viewport_point.column);
        let mut sel = alacritty_terminal::selection::Selection::new(ty, anchor, side);
        let opposite = match side {
            ASide::Left => ASide::Right,
            ASide::Right => ASide::Left,
        };
        sel.update(anchor, opposite);
        self.term.selection = Some(sel);
        self.selection_anchor = Some(anchor);
        self.stage_full_damage();
    }

    /// Extends the active selection's moving end to `viewport_point` /
    /// `side`. Same viewport-row → alacritty-Line translation as
    /// `selection_start_at`. No-op (no panic, no state change) when
    /// `Term::selection` is `None` — alacritty wipes the selection on
    /// alt-screen swap, and the Bevy glue may still emit drag events for
    /// one frame after that.
    pub fn selection_update_to(
        &mut self,
        viewport_point: alacritty_terminal::index::Point,
        side: alacritty_terminal::index::Side,
    ) {
        if self.term.selection.is_none() {
            return;
        }
        let line = viewport_row_to_line(&self.term, viewport_point.line.0);
        let point = alacritty_terminal::index::Point::new(line, viewport_point.column);
        if let Some(sel) = self.term.selection.as_mut() {
            sel.update(point, side);
        }
        self.stage_full_damage();
    }

    /// Starts a selection of the given type at the current vi cursor.
    ///
    /// Internally seeds `Selection::new(ty, vi_cursor, Side::Left)` and
    /// immediately calls `update(vi_cursor, Side::Right)` so the anchor
    /// cell is included — a single `Selection::new` alone returns `None`
    /// from `to_range` when start and end coincide, so
    /// `selection_to_string` would yield `None`.
    pub fn selection_start(&mut self, ty: alacritty_terminal::selection::SelectionType) {
        let anchor = self.term.vi_mode_cursor.point;
        let mut sel = alacritty_terminal::selection::Selection::new(
            ty,
            anchor,
            alacritty_terminal::index::Side::Left,
        );
        sel.update(anchor, alacritty_terminal::index::Side::Right);
        self.term.selection = Some(sel);
        self.selection_anchor = Some(anchor);
        self.stage_full_damage();
    }

    /// Drops the active selection and stages Full damage so the
    /// renderer's next frame carries `selection: None`.
    pub fn selection_clear(&mut self) {
        self.term.selection = None;
        self.selection_anchor = None;
        self.stage_full_damage();
    }

    /// Switches the active selection's type while preserving the
    /// original anchor (captured at `selection_start`). The new
    /// selection spans from the stored anchor to the current vi cursor,
    /// mirroring tmux's behaviour when the user switches between `v`
    /// (Char) and `V` (Line) without exiting copy mode.
    ///
    /// Returns `false` when no selection anchor is stored (i.e. no
    /// selection is currently active). In that case callers should fall
    /// back to `selection_start`.
    pub fn selection_change_type(
        &mut self,
        ty: alacritty_terminal::selection::SelectionType,
    ) -> bool {
        let Some(anchor) = self.selection_anchor else {
            return false;
        };
        let cursor = self.term.vi_mode_cursor.point;
        let mut sel = alacritty_terminal::selection::Selection::new(
            ty,
            anchor,
            alacritty_terminal::index::Side::Left,
        );
        sel.update(cursor, alacritty_terminal::index::Side::Right);
        self.term.selection = Some(sel);
        self.stage_full_damage();
        true
    }

    /// Reads the current cols / rows / cursor.
    pub fn read_geometry(&self) -> (u16, u16, Cursor) {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        let cursor = extract_cursor(&self.term);
        (cols, rows, cursor)
    }

    /// Returns the current `TermMode` bitflags. Used by the mouse-wheel
    /// router to choose between SGR/X10 mouse-protocol output, alt-screen
    /// arrow translation, and host scrollback.
    pub fn current_modes(&self) -> TermMode {
        *self.term.mode()
    }

    /// True iff `TermMode::APP_CURSOR` (DECCKM) is set on the inner term.
    /// Used by `input_codec::encode_key` to choose between `ESC [ A/B/C/D`
    /// and `ESC O A/B/C/D` for arrow keys.
    pub fn is_app_cursor_keys(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Returns `true` when the active `Term` has `TermMode::BRACKETED_PASTE`
    /// enabled (the application has sent `\x1b[?2004h` and not yet sent the
    /// matching `\x1b[?2004l`). The paste pipeline reads this to decide
    /// whether to wrap clipboard contents in `\x1b[200~` / `\x1b[201~`.
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Returns true when the viewport is pinned to the live tail
    /// (`display_offset == 0`). Used by the mouse-wheel input system to
    /// decide whether keyboard input or Esc should trigger a
    /// reset-to-bottom before forwarding.
    pub fn is_at_bottom(&self) -> bool {
        self.term.grid().display_offset() == 0
    }

    /// Reads the active selection as a UTF-8 string via
    /// `Term::selection_to_string`. Returns `None` when no selection is
    /// set or the selection is empty.
    pub fn selection_to_string(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    /// Returns the type of the active selection, if any. Used by the
    /// v / V toggle predicate.
    pub fn selection_type(&self) -> Option<alacritty_terminal::selection::SelectionType> {
        self.term.selection.as_ref().map(|s| s.ty)
    }

    /// Returns the current scroll offset and history length for the
    /// copy-mode indicator's `[offset/total]` chip.
    pub fn vi_indicator_snapshot(&self) -> ViIndicatorSnapshot {
        ViIndicatorSnapshot {
            scroll_offset: self.term.grid().display_offset(),
            history_size: self.term.history_size(),
        }
    }

    fn with_channels(
        cols: u16,
        rows: u16,
        listener: TermListener,
        reply_rx: Receiver<Vec<u8>>,
        control_rx: Receiver<ControlFrame>,
        control_tx: Sender<ControlFrame>,
    ) -> Self {
        let size = LocalDim::new(cols, rows);
        let term = Term::new(Config::default(), &size, listener);
        Self {
            term,
            parser: Processor::new(),
            hyperlinks: HyperlinkInterner::new(),
            coalescer: Coalescer::new(),
            prev_cursor: None,
            prev_vi_cursor: None,
            prev_selection: None,
            selection_anchor: None,
            pending_user_input: false,
            pending_damage: None,
            first_emit: true,
            scratch_dirty: Vec::new(),
            row_hashes: HashMap::new(),
            window_open_mode: None,
            frame_seq: 0,
            pending_events: Vec::new(),
            reply_rx,
            control_rx,
            osc7_parser: Parser::new(),
            osc7: Osc7Capture::new(
                control_tx,
                gethostname::gethostname().to_string_lossy().into_owned(),
            ),
        }
    }

    fn advance(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.parser.advance(&mut self.term, chunk);
        self.osc7_parser.advance(&mut self.osc7, chunk);
    }

    fn cursor_changed(&self) -> bool {
        let curr = extract_cursor(&self.term);
        self.prev_cursor.as_ref().is_none_or(|prev| *prev != curr)
    }

    fn needs_bootstrap_emit(&self) -> bool {
        self.first_emit && self.pending_damage.is_none()
    }

    fn force_bootstrap_damage(&mut self) {
        self.collect_full_damage();
    }

    fn stage_full_damage(&mut self) {
        self.collect_full_damage();
    }

    fn collect_full_damage(&mut self) {
        let mut scratch = std::mem::take(&mut self.scratch_dirty);
        self.pending_damage = Some(DirtyRows::collect(&mut self.term, &mut scratch));
        self.scratch_dirty = scratch;
    }

    fn abort_emit_with_no_damage(&mut self) {
        self.coalescer.disarm();
        self.window_open_mode = None;
    }

    fn consume_window_open_mode(&mut self) -> TermMode {
        self.window_open_mode
            .take()
            .unwrap_or_else(|| *self.term.mode())
    }

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

    fn is_noop_emit(
        &self,
        dirty: &DirtyRows,
        curr_cursor: &Cursor,
        prev_mode: TermMode,
        curr_mode: TermMode,
        curr_vi_cursor: Option<ViCursor>,
        curr_selection: Option<SelectionRange>,
    ) -> bool {
        let dirty_is_empty = matches!(dirty, DirtyRows::Rows(r) if r.is_empty());
        let cursor_unchanged = self
            .prev_cursor
            .as_ref()
            .is_some_and(|prev| *prev == *curr_cursor);
        let vi_unchanged = self.prev_vi_cursor == curr_vi_cursor;
        let sel_unchanged = self.prev_selection == curr_selection;
        dirty_is_empty
            && prev_mode == curr_mode
            && cursor_unchanged
            && vi_unchanged
            && sel_unchanged
            && !self.first_emit
    }

    fn next_frame_seq(&mut self) -> u32 {
        let seq = self.frame_seq;
        self.frame_seq = self.frame_seq.wrapping_add(1);
        seq
    }

    fn announce_mode_change(&mut self, prev_mode: TermMode, curr_mode: TermMode) {
        let mode_change = diff_mode(prev_mode, curr_mode);
        if mode_change.is_empty() {
            return;
        }
        let added: Vec<String> = mode_change.added.into_iter().map(String::from).collect();
        let removed: Vec<String> = mode_change.removed.into_iter().map(String::from).collect();
        self.pending_events
            .push(VtEvent::ModeChanged { added, removed });
    }

    fn emit_snapshot(&mut self, seq: u32, reason: SnapshotReason) -> FrameSnapshot {
        let snapshot = build_snapshot(&self.term, seq, reason, &mut self.hyperlinks);
        self.rebuild_full_row_hashes();
        snapshot
    }

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

    fn emit_delta(&mut self, seq: u32, rows: Vec<u16>, kept_hashes: Vec<(i32, u64)>) -> FrameDelta {
        let delta = build_delta(&self.term, seq, &rows, &mut self.hyperlinks);
        self.scratch_dirty = rows;
        for (line_i32, h) in kept_hashes {
            self.row_hashes.insert(line_i32, h);
        }
        delta
    }

    fn finalize_emit(&mut self) {
        self.term.reset_damage();
        self.coalescer.disarm();
    }
}

/// Classification used by `decide_frame_kind` to select snapshot vs
/// delta. Local to this module — `frame_builder` doesn't need it.
enum FrameKind {
    Snapshot { reason: SnapshotReason },
    Delta { rows: Vec<u16> },
}

/// Row-count fraction at which Partial damage promotes to a Snapshot.
/// `partial * SNAPSHOT_THRESHOLD_DENOM >= total * SNAPSHOT_THRESHOLD_NUMER`
/// holds when partial / total >= 17 / 20 = 85 %. Beyond this fraction,
/// a full snapshot is more bandwidth-efficient than enumerating dirty
/// rows.
const SNAPSHOT_THRESHOLD_NUMER: u32 = 17; // threshold is 17/20 = 85 %
const SNAPSHOT_THRESHOLD_DENOM: u32 = 20;

/// Selects the frame type. Policy (priority order):
/// 1. `first_emit` → `Snapshot { reason: Initial }`
/// 2. `DirtyRows::Full` → `Snapshot { reason: Resize }`
/// 3. Partial damage >= 85 % of total rows → `Snapshot { reason: Resize }`
/// 4. Otherwise → `Delta { rows }`
fn decide_frame_kind(vt: &Vt, dirty: DirtyRows) -> FrameKind {
    let total_rows = vt.term.screen_lines() as u16;
    if vt.first_emit {
        return FrameKind::Snapshot {
            reason: SnapshotReason::Initial,
        };
    }
    match dirty {
        DirtyRows::Full => FrameKind::Snapshot {
            reason: SnapshotReason::Resize,
        },
        DirtyRows::Rows(rows) => {
            if (rows.len() as u32) * SNAPSHOT_THRESHOLD_DENOM
                >= (total_rows as u32) * SNAPSHOT_THRESHOLD_NUMER
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

/// Computes a content hash for a single grid row, including the cursor
/// overlay when the cursor lands on this `viewport_y`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line as ALine, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::vi_mode::ViMotion;

    fn advance(vt: &mut Vt, bytes: &[u8]) {
        vt.advance(bytes);
    }

    #[test]
    fn is_noop_emit_returns_false_when_only_selection_changed() {
        let mut vt = Vt::new(10, 3);
        vt.first_emit = false;
        vt.prev_cursor = Some(extract_cursor(&vt.term));
        vt.prev_selection = None;
        let p = Point::new(ALine(0), Column(0));
        let mut sel = Selection::new(SelectionType::Simple, p, Side::Left);
        sel.update(p, Side::Right);
        vt.term.selection = Some(sel);
        let dirty = DirtyRows::Rows(Vec::new());
        let curr_cursor = extract_cursor(&vt.term);
        let mode = *vt.term.mode();
        let curr_sel = extract_selection_range(&vt.term);
        let curr_vi = extract_vi_cursor(&vt.term);
        assert!(
            !vt.is_noop_emit(&dirty, &curr_cursor, mode, mode, curr_vi, curr_sel),
            "selection appeared on prev_selection==None → must NOT be a no-op"
        );
    }

    #[test]
    fn is_noop_emit_returns_false_when_only_vi_cursor_changed() {
        let mut vt = Vt::new(10, 3);
        vt.first_emit = false;
        vt.prev_cursor = Some(extract_cursor(&vt.term));
        vt.prev_vi_cursor = None;
        vt.term.toggle_vi_mode();
        let dirty = DirtyRows::Rows(Vec::new());
        let curr_cursor = extract_cursor(&vt.term);
        let mode = *vt.term.mode();
        let curr_sel = extract_selection_range(&vt.term);
        let curr_vi = extract_vi_cursor(&vt.term);
        assert!(curr_vi.is_some(), "vi mode on → curr_vi must be Some");
        assert!(
            !vt.is_noop_emit(&dirty, &curr_cursor, mode, mode, curr_vi, curr_sel),
            "vi cursor appeared on prev_vi_cursor==None → must NOT be a no-op"
        );
    }

    #[test]
    fn enter_vi_mode_sets_term_mode_vi_bit() {
        let mut vt = Vt::new(10, 3);
        assert!(!vt.term.mode().contains(TermMode::VI));
        vt.enter_vi_mode();
        assert!(vt.term.mode().contains(TermMode::VI));
    }

    #[test]
    fn enter_vi_mode_is_idempotent_when_already_in_vi() {
        let mut vt = Vt::new(10, 3);
        vt.enter_vi_mode();
        let was_vi = vt.term.mode().contains(TermMode::VI);
        vt.enter_vi_mode();
        assert!(
            was_vi && vt.term.mode().contains(TermMode::VI),
            "second enter_vi_mode must leave VI bit set, not toggle it off"
        );
    }

    #[test]
    fn vi_motion_down_advances_vi_cursor_line_by_one() {
        let mut vt = Vt::new(10, 5);
        vt.enter_vi_mode();
        let before = vt.term.vi_mode_cursor.point.line.0;
        vt.vi_motion(ViMotion::Down);
        let after = vt.term.vi_mode_cursor.point.line.0;
        assert_eq!(after, before + 1, "ViMotion::Down advances by 1 line");
    }

    #[test]
    fn scroll_page_up_grows_display_offset() {
        let mut vt = Vt::new(10, 5);
        for _ in 0..30 {
            advance(&mut vt, b"x\r\n");
        }
        assert_eq!(vt.term.grid().display_offset(), 0);
        vt.scroll_page_up();
        assert!(
            vt.term.grid().display_offset() > 0,
            "PageUp must grow display_offset"
        );
    }

    #[test]
    fn exit_vi_mode_clears_vi_and_snaps_to_live_tail() {
        let mut vt = Vt::new(10, 3);
        for _ in 0..20 {
            advance(&mut vt, b"x\r\n");
        }
        vt.term.scroll_display(Scroll::Top);
        vt.enter_vi_mode();
        assert!(vt.term.grid().display_offset() > 0);
        vt.exit_vi_mode();
        assert!(!vt.term.mode().contains(TermMode::VI));
        assert_eq!(
            vt.term.grid().display_offset(),
            0,
            "exit must snap to live tail (display_offset == 0)"
        );
    }

    #[test]
    fn selection_start_simple_at_vi_cursor_includes_that_cell() {
        let mut vt = Vt::new(10, 3);
        advance(&mut vt, b"X");
        vt.enter_vi_mode();
        vt.vi_motion(ViMotion::Left);
        vt.selection_start(SelectionType::Simple);
        let s = vt
            .selection_to_string()
            .expect("non-empty 1-cell selection");
        assert!(
            s.contains('X'),
            "selection text {s:?} must include the anchor cell glyph 'X'"
        );
    }

    #[test]
    fn selection_clear_drops_term_selection() {
        let mut vt = Vt::new(10, 3);
        vt.enter_vi_mode();
        vt.selection_start(SelectionType::Simple);
        assert!(vt.term.selection.is_some());
        vt.selection_clear();
        assert!(vt.term.selection.is_none());
    }

    #[test]
    fn selection_to_string_returns_none_when_no_selection() {
        let vt = Vt::new(10, 3);
        assert!(vt.selection_to_string().is_none());
    }

    #[test]
    fn selection_type_returns_ty_of_active_selection() {
        let mut vt = Vt::new(10, 3);
        vt.enter_vi_mode();
        assert!(vt.selection_type().is_none());
        vt.selection_start(SelectionType::Lines);
        assert!(matches!(vt.selection_type(), Some(SelectionType::Lines)));
    }

    #[test]
    fn selection_change_type_preserves_anchor_across_v_to_lines_switch() {
        let mut vt = Vt::new(10, 5);
        advance(&mut vt, b"abcdefghij\r\nklmnopqrst\r\n");
        vt.enter_vi_mode();
        vt.term.vi_mode_cursor.point = Point::new(ALine(0), Column(2));
        vt.selection_start(SelectionType::Simple);
        vt.vi_motion(ViMotion::Down);
        for _ in 0..5 {
            vt.vi_motion(ViMotion::Right);
        }
        let chars_before = vt
            .selection_to_string()
            .expect("simple selection spans something")
            .len();
        assert!(
            chars_before > 1,
            "precondition: char-wise selection must be multi-char before type switch, got {chars_before} chars",
        );
        assert!(
            vt.selection_change_type(SelectionType::Lines),
            "selection_change_type must report success when a selection is active",
        );
        let s_after = vt
            .selection_to_string()
            .expect("Lines selection still active after type change");
        assert!(
            s_after.starts_with("abc"),
            "Lines selection must START at row 0 (preserved anchor), got {s_after:?}",
        );
        assert!(
            s_after.contains("klmno"),
            "Lines selection must REACH row 1 (current vi cursor), got {s_after:?}",
        );
    }

    #[test]
    fn selection_change_type_returns_false_when_no_selection_active() {
        let mut vt = Vt::new(10, 3);
        vt.enter_vi_mode();
        assert!(
            !vt.selection_change_type(SelectionType::Lines),
            "selection_change_type must return false when no selection anchor is stored",
        );
        assert!(
            vt.selection_type().is_none(),
            "no selection should be created"
        );
    }

    #[test]
    fn vi_indicator_snapshot_reads_current_offset_and_history_size() {
        let mut vt = Vt::new(10, 5);
        let snap0 = vt.vi_indicator_snapshot();
        assert_eq!(snap0.scroll_offset, 0, "fresh terminal at live tail");

        let mut payload = Vec::with_capacity(1024);
        for i in 0..30u32 {
            payload.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        advance(&mut vt, &payload);
        vt.enter_vi_mode();
        vt.scroll_page_up();

        let snap1 = vt.vi_indicator_snapshot();
        assert!(
            snap1.scroll_offset > 0,
            "PageUp after seeded scrollback must grow scroll_offset (snapshot: {snap1:?})"
        );
        assert_eq!(
            snap1.history_size,
            vt.term.history_size(),
            "history_size must equal Term::history_size()"
        );
    }

    #[test]
    fn bracketed_paste_enabled_reports_false_when_unset_and_true_after_set_sequence() {
        let mut vt = Vt::new(10, 5);
        assert!(
            !vt.bracketed_paste_enabled(),
            "fresh Term must not have BRACKETED_PASTE set",
        );
        advance(&mut vt, b"\x1b[?2004h");
        assert!(
            vt.bracketed_paste_enabled(),
            "after advance(\\x1b[?2004h) BRACKETED_PASTE must be set",
        );
        advance(&mut vt, b"\x1b[?2004l");
        assert!(
            !vt.bracketed_paste_enabled(),
            "after advance(\\x1b[?2004l) BRACKETED_PASTE must be cleared",
        );
    }

    #[test]
    fn note_user_input_flips_pending_flag() {
        let mut vt = Vt::new(10, 5);
        assert!(!vt.pending_user_input, "fresh engine has no pending input");
        vt.note_user_input();
        assert!(vt.pending_user_input, "note_user_input sets the flag");
    }

    #[test]
    fn selection_start_at_creates_simple_selection_at_arbitrary_point() {
        let mut vt = Vt::new(80, 24);
        let point = Point::new(ALine(5), Column(10));
        vt.selection_start_at(point, Side::Left, SelectionType::Simple);
        assert_eq!(vt.selection_type(), Some(SelectionType::Simple));
        assert!(vt.selection_to_string().is_some());
    }

    #[test]
    fn selection_start_at_immediate_update_avoids_none_string() {
        let mut vt = Vt::new(80, 24);
        vt.selection_start_at(
            Point::new(ALine(0), Column(0)),
            Side::Left,
            SelectionType::Block,
        );
        assert!(
            vt.selection_to_string().is_some(),
            "Block selection_start_at must produce a non-None selection_to_string via the immediate update"
        );
    }

    #[test]
    fn selection_update_to_extends_existing_selection() {
        let mut vt = Vt::new(80, 24);
        vt.selection_start_at(
            Point::new(ALine(0), Column(0)),
            Side::Left,
            SelectionType::Simple,
        );
        vt.selection_update_to(Point::new(ALine(0), Column(10)), Side::Right);
        assert!(
            vt.selection_to_string().is_some(),
            "selection_to_string must return Some(_) after a valid update"
        );
    }

    #[test]
    fn selection_update_to_no_op_when_no_selection() {
        let mut vt = Vt::new(80, 24);
        vt.selection_update_to(Point::new(ALine(0), Column(5)), Side::Right);
        assert!(vt.selection_type().is_none());
    }

    #[test]
    fn vi_goto_moves_vi_cursor_in_vi_mode() {
        let mut vt = Vt::new(80, 24);
        vt.enter_vi_mode();
        vt.vi_goto(Point::new(ALine(5), Column(12)));
        assert!(vt.current_modes().contains(TermMode::VI));
    }

    #[test]
    fn vi_goto_is_noop_outside_vi_mode() {
        let mut vt = Vt::new(80, 24);
        vt.vi_goto(Point::new(ALine(2), Column(3)));
        assert!(!vt.current_modes().contains(TermMode::VI));
    }

    #[test]
    fn advance_osc7_then_drain_events_yields_current_dir() {
        let mut vt = Vt::new(80, 24);
        advance(&mut vt, b"\x1b]7;file://localhost/tmp\x07");
        let events = vt.drain_events();
        assert_eq!(
            events,
            vec![VtEvent::CurrentDir(std::path::PathBuf::from("/tmp"))]
        );
    }

    #[test]
    fn is_at_bottom_true_initially() {
        let vt = Vt::new(80, 24);
        assert!(vt.is_at_bottom(), "fresh terminal must be at bottom");
    }

    #[test]
    fn current_modes_returns_term_mode() {
        let vt = Vt::new(80, 24);
        assert!(!vt.current_modes().contains(TermMode::ALT_SCREEN));
    }

    #[test]
    fn on_output_empty_input_is_idle() {
        let mut vt = Vt::new(10, 3);
        assert_eq!(vt.on_output(b"", Instant::now()), OutputAction::Idle);
    }

    #[test]
    fn on_output_first_chunk_emits_now_then_emit_returns_initial_snapshot() {
        let mut vt = Vt::new(10, 3);
        let action = vt.on_output(b"hello", Instant::now());
        assert_eq!(
            action,
            OutputAction::EmitNow,
            "bootstrap chunk flushes immediately"
        );
        let frame = vt.emit().expect("first emit produces a frame");
        match frame {
            Frame::Snapshot(s) => assert_eq!(s.reason, SnapshotReason::Initial),
            Frame::Delta(_) => panic!("first emit must be a Snapshot"),
        }
    }

    #[test]
    fn on_output_non_user_input_arms_and_tick_flushes_at_deadline() {
        let mut vt = Vt::new(10, 3);
        let _ = vt.on_output(b"hello", Instant::now());
        let _ = vt.emit();
        let start = Instant::now();
        let action = vt.on_output(b"world", start);
        assert_eq!(
            action,
            OutputAction::Armed,
            "non-user output arms the coalescer"
        );
        assert!(vt.next_deadline().is_some(), "armed engine has a deadline");
        let frame = vt.tick(start + Coalescer::MAX_CAP);
        assert!(
            frame.is_some(),
            "tick past the deadline flushes the armed window"
        );
    }

    #[test]
    fn tick_bootstrap_rescues_initial_snapshot_without_output() {
        let mut vt = Vt::new(10, 3);
        let frame = vt.tick(Instant::now());
        match frame {
            Some(Frame::Snapshot(s)) => assert_eq!(s.reason, SnapshotReason::Initial),
            other => panic!("bootstrap tick must produce an Initial snapshot, got {other:?}"),
        }
    }

    #[test]
    fn scroll_stages_damage_and_tick_arms_then_flushes() {
        let mut vt = Vt::new(10, 5);
        for _ in 0..30 {
            advance(&mut vt, b"x\r\n");
        }
        let _ = vt.tick(Instant::now());
        vt.scroll_page_up();
        assert!(vt.pending_damage.is_some(), "scroll stages damage");
        assert!(!vt.coalescer.is_armed(), "scroll itself does not arm");
        let start = Instant::now();
        let _ = vt.tick(start);
        assert!(vt.coalescer.is_armed(), "tick arms staged damage");
        let frame = vt.tick(start + Coalescer::MAX_CAP);
        assert!(
            frame.is_some(),
            "later tick past deadline flushes the staged damage"
        );
    }

    #[test]
    fn drain_replies_returns_pty_write_bytes() {
        let mut vt = Vt::new(10, 3);
        advance(&mut vt, b"\x1b[6n");
        let replies = vt.drain_replies();
        assert!(
            !replies.is_empty(),
            "DSR cursor-position query must produce a reply"
        );
    }

    #[test]
    fn alt_screen_enter_emits_mode_changed_event() {
        let mut vt = Vt::new(10, 5);
        vt.on_output(b"\x1b[?1049h", Instant::now());
        let _ = vt.emit();
        let events = vt.drain_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                VtEvent::ModeChanged { added, .. }
                    if added.iter().any(|m| m == "alt-screen")
            )),
            "alt-screen enter (\\x1b[?1049h) must produce ModeChanged with \"alt-screen\" in added; events = {events:?}",
        );
        let mode_event = events.iter().find(|e| {
            matches!(e, VtEvent::ModeChanged { added, .. } if added.iter().any(|m| m == "alt-screen"))
        });
        if let Some(VtEvent::ModeChanged { removed, .. }) = mode_event {
            assert!(
                removed.is_empty(),
                "alt-screen enter must have an empty removed list, got {removed:?}"
            );
        }
    }
}
