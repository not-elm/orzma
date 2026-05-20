//! Pure functions that turn a Term snapshot into wire frames.
//!
//! All entry points take the Term by mutable reference because
//! `Term::damage()` mutates internal damage state. Callers must hold the
//! `vt_state` lock; this module performs no locking.

use crate::vt::frame::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Hyperlink, Row, Run,
    SnapshotReason, style,
};
use crate::vt::hyperlink::{HyperlinkInterner, HyperlinkUri, HyperlinkWireId};
use crate::vt::mode_diff::TRACKED_MODES;
use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermDamage;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
use unicode_width::UnicodeWidthChar;

/// Outcome of damage inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRows {
    /// Entire screen is dirty (resize / alt-screen swap / clear / reset).
    Full,
    /// Specific row indices are dirty.
    Rows(Vec<u16>),
}

/// Reads the damage tracker and returns row indices that changed.
///
/// `&mut Term` is required because `Term::damage()` takes `&mut self`.
/// `scratch_dirty` is cleared, filled with the dirty row indices, then
/// moved into the returned `DirtyRows::Rows` variant via `mem::take`.
/// The caller should reclaim the consumed `Vec` back into the scratch field
/// after the emit completes so capacity persists across calls.
pub(super) fn collect_dirty_rows<T>(term: &mut Term<T>, scratch_dirty: &mut Vec<u16>) -> DirtyRows {
    match term.damage() {
        TermDamage::Full => DirtyRows::Full,
        TermDamage::Partial(iter) => {
            scratch_dirty.clear();
            scratch_dirty.extend(iter.map(|d| d.line as u16));
            DirtyRows::Rows(std::mem::take(scratch_dirty))
        }
    }
}

/// Builds a full-screen snapshot frame.
///
/// Reads the current terminal grid via shared reference, coalescing each row
/// into runs of cells with identical attributes. Wide-char spacer cells are
/// skipped so the wide character itself accounts for its full column width.
#[tracing::instrument(skip_all, fields(seq))]
pub(crate) fn build_snapshot<T>(
    term: &Term<T>,
    seq: u32,
    reason: SnapshotReason,
    interner: &mut HyperlinkInterner,
    produced_at_us: Option<u64>,
) -> FrameSnapshot {
    let cols = term.columns() as u16;
    let rows = term.screen_lines() as u16;
    let mut hyperlinks_opt: Option<Vec<(HyperlinkWireId, HyperlinkUri)>> = None;
    let rows_data: Vec<Row> = (0..rows as i32)
        .map(|y| Row {
            runs: coalesce_row(term, y, interner, &mut hyperlinks_opt),
        })
        .collect();
    FrameSnapshot {
        seq,
        cols,
        rows,
        cursor: extract_cursor(term),
        rows_data,
        reason,
        modes: snapshot_modes(*term.mode()),
        hyperlinks: hyperlinks_opt
            .unwrap_or_default()
            .into_iter()
            .map(|(id, uri)| Hyperlink { id, uri })
            .collect(),
        display_offset: term.grid().display_offset() as u32,
        history_size: term.history_size() as u32,
        produced_at_us,
    }
}

/// Builds a delta frame containing only the listed dirty rows.
///
/// Each entry is a full-row replacement (not partial). Row ordering follows
/// the supplied slice.
#[tracing::instrument(skip_all, fields(seq))]
pub(super) fn build_delta<T>(
    term: &Term<T>,
    seq: u32,
    rows: &[u16],
    interner: &mut HyperlinkInterner,
    produced_at_us: Option<u64>,
    modes_added: Vec<String>,
    modes_removed: Vec<String>,
) -> FrameDelta {
    let mut hyperlinks_opt: Option<Vec<(HyperlinkWireId, HyperlinkUri)>> = None;
    let dirty_rows: Vec<DirtyRow> = rows
        .iter()
        .map(|&r| DirtyRow {
            row: r,
            runs: coalesce_row(term, r as i32, interner, &mut hyperlinks_opt),
        })
        .collect();
    FrameDelta {
        seq,
        cursor: extract_cursor(term),
        dirty_rows,
        hyperlinks: hyperlinks_opt
            .unwrap_or_default()
            .into_iter()
            .map(|(id, uri)| Hyperlink { id, uri })
            .collect(),
        display_offset: term.grid().display_offset() as u32,
        produced_at_us,
        modes_added,
        modes_removed,
    }
}

pub(crate) fn extract_cursor<T>(term: &Term<T>) -> Cursor {
    let point = term.grid().cursor.point;
    let mut x = point.column.0 as u16;
    // NOTE: alacritty's RenderableCursor shifts x left by 1 when the cursor
    // lands on a wide-char spacer; replicate so the visible cursor aligns
    // with the wide glyph itself. Use live-grid line for the spacer check —
    // the cursor is always tracked in live-grid coordinates regardless of
    // display_offset.
    let cell = &term.grid()[Line(point.line.0)][point.column];
    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && x > 0 {
        x -= 1;
    }
    // NOTE: cursor.point.line is in live-grid coordinates (0..screen_lines).
    // Convert to viewport coordinates by adding display_offset. When the
    // result falls outside the visible viewport we hide the cursor, otherwise
    // the live-tail caret would float over scrolled-out history rows.
    let display_offset = term.grid().display_offset() as i32;
    let screen_lines = term.screen_lines() as i32;
    let viewport_y = point.line.0 + display_offset;
    let in_viewport = (0..screen_lines).contains(&viewport_y);
    let y = if in_viewport { viewport_y as u16 } else { 0 };
    // NOTE: DECSCUSR shape selection. `HollowBlock` (CSI 0/1 SP q in some
    // variants) maps to Block; alacritty distinguishes them but our wire
    // vocabulary does not — clients render the same overlay.
    let style = term.cursor_style();
    let shape = match style.shape {
        alacritty_terminal::vte::ansi::CursorShape::Block
        | alacritty_terminal::vte::ansi::CursorShape::HollowBlock => CursorShape::Block,
        alacritty_terminal::vte::ansi::CursorShape::Underline => CursorShape::Underline,
        alacritty_terminal::vte::ansi::CursorShape::Beam => CursorShape::Bar,
        alacritty_terminal::vte::ansi::CursorShape::Hidden => CursorShape::Block,
    };
    Cursor {
        x,
        y,
        shape,
        blinking: style.blinking,
        // NOTE: DECTCEM (`\033[?25l/h`) toggles `TermMode::SHOW_CURSOR` — used
        // by vim/less/fzf for cursor hiding. DECSCUSR `Hidden` shape is a
        // separate concept. `in_viewport` gates visibility when the user has
        // scrolled the cursor's line out of view. All three must hold for the
        // cursor to render.
        visible: term.mode().contains(TermMode::SHOW_CURSOR)
            && style.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden
            && in_viewport,
    }
}

/// Returns the alacritty `Line` currently displayed at viewport row `y`.
///
/// Translates the wire-protocol viewport coordinate `y` (0..screen_lines)
/// into the active grid's line index using the current `display_offset`.
/// Equivalent to alacritty's `viewport_to_point(display_offset, ..).line`.
///
/// Negative `Line` values are legal here: alacritty's ring storage maps
/// them to scrollback history (`Line(-1)` = most recent scrolled-out
/// line). Both bounds of `Storage::compute_index` are satisfied because
/// alacritty guarantees `display_offset <= history_size <= max_scroll_limit`.
///
/// # Invariants (caller-side)
/// - `0 <= y < screen_lines`
/// - `display_offset` fits in `i32`. ozmux uses the default
///   `scrolling_history = 10000`, far below `i32::MAX`.
pub(super) fn viewport_row_to_line<T>(term: &Term<T>, y: i32) -> Line {
    debug_assert!(
        (0..term.screen_lines() as i32).contains(&y),
        "viewport row {y} out of range 0..{}",
        term.screen_lines(),
    );
    let off = term.grid().display_offset() as i32;
    Line(y - off)
}

fn snapshot_modes(curr: TermMode) -> Vec<String> {
    TRACKED_MODES
        .iter()
        .filter(|(flag, _)| curr.contains(*flag))
        .map(|(_, name)| (*name).to_string())
        .collect()
}

/// Coalesces a row's cells into runs of identical attributes.
///
/// Accepts `&mut HyperlinkInterner` (not `&mut VtState`) so the caller can
/// split-borrow `&term` and `&mut interner` from `VtState` disjointly.
/// `emitted_hyperlinks` accumulates `(wire_id, uri)` pairs encountered in this
/// row — the caller merges across all coalesced rows to produce the wire
/// `FrameSnapshot.hyperlinks` / `FrameDelta.hyperlinks`.
///
/// # Invariants
/// - `y` is a viewport row in `0..screen_lines` (matches wire-protocol
///   `row` semantics); callers pass the raw viewport coordinate and the
///   grid translation happens internally via `viewport_row_to_line`.
/// - The returned runs reflect the grid line currently displayed at
///   viewport row `y`, taking the active grid's `display_offset` into
///   account.
fn coalesce_row<T>(
    term: &Term<T>,
    y: i32,
    interner: &mut HyperlinkInterner,
    emitted_hyperlinks: &mut Option<Vec<(HyperlinkWireId, HyperlinkUri)>>,
) -> Vec<Run> {
    let cols = term.columns() as u16;
    let grid_row = &term.grid()[viewport_row_to_line(term, y)];
    let mut runs: Vec<Run> = Vec::new();
    let mut acc_text = String::new();
    let mut acc_cols: u16 = 0;
    let mut acc_attrs: Option<RunAttrs> = None;

    for x in 0..cols {
        let cell = &grid_row[Column(x as usize)];
        // NOTE: spacer cells exist for grid alignment only; their `c` field
        // duplicates the leading wide glyph or holds U+0020. Skip both
        // variants so the wide char's run accounts for the full 2 columns.
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        // Resolve hyperlink (alac String id → HyperlinkWireId via interner).
        let hyperlink_id = cell.hyperlink().map(|h| {
            let id = interner.intern(h.id(), h.uri().as_ref());
            let v = emitted_hyperlinks.get_or_insert_with(Vec::new);
            if !v.iter().any(|(k, _)| *k == id) {
                v.push((id, HyperlinkUri::new(h.uri().to_string())));
            }
            id
        });
        let cell_attrs = RunAttrs::from_cell(cell, hyperlink_id);
        // NOTE: alacritty represents an unallocated cell as `'\0'`. Treat it as
        // a width-1 space so the renderer always paints a background for every
        // grid column; otherwise width-0 NUL cells skip bg fill on the client
        // and the parent pane bleeds through.
        let glyph = if cell.c == '\0' { ' ' } else { cell.c };
        let cell_width = char_width(glyph);
        match &acc_attrs {
            Some(prev) if *prev == cell_attrs => {
                acc_text.push(glyph);
                acc_cols += cell_width;
            }
            _ => {
                if let Some(attrs) = acc_attrs.take() {
                    runs.push(attrs.into_run(
                        std::mem::take(&mut acc_text),
                        std::mem::replace(&mut acc_cols, 0),
                    ));
                }
                acc_attrs = Some(cell_attrs);
                acc_text.push(glyph);
                acc_cols = cell_width;
            }
        }
    }
    if let Some(attrs) = acc_attrs {
        runs.push(attrs.into_run(acc_text, acc_cols));
    }
    runs
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunAttrs {
    fg: Color,
    bg: Color,
    style: u8,
    hyperlink_id: Option<HyperlinkWireId>,
}

impl RunAttrs {
    fn from_cell(cell: &Cell, hyperlink_id: Option<HyperlinkWireId>) -> Self {
        Self {
            fg: color_from_alacritty(cell.fg),
            bg: color_from_alacritty(cell.bg),
            style: style_from_flags(cell.flags),
            hyperlink_id,
        }
    }

    fn into_run(self, text: String, cols: u16) -> Run {
        Run {
            cols,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
            text,
            hyperlink_id: self.hyperlink_id,
        }
    }
}

fn color_from_alacritty(c: AColor) -> Color {
    match c {
        AColor::Named(NamedColor::Foreground) | AColor::Named(NamedColor::Background) => {
            Color::Default
        }
        AColor::Named(named) => Color::Indexed(named as u8),
        AColor::Indexed(i) => Color::Indexed(i),
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

fn style_from_flags(flags: Flags) -> u8 {
    let mut s: u8 = 0;
    if flags.contains(Flags::BOLD) {
        s |= style::BOLD;
    }
    if flags.contains(Flags::ITALIC) {
        s |= style::ITALIC;
    }
    if flags.contains(Flags::UNDERLINE) {
        s |= style::UNDERLINE;
    }
    if flags.contains(Flags::STRIKEOUT) {
        s |= style::STRIKE;
    }
    if flags.contains(Flags::INVERSE) {
        s |= style::REVERSE;
    }
    if flags.contains(Flags::DIM) {
        s |= style::DIM;
    }
    s
}

fn char_width(c: char) -> u16 {
    UnicodeWidthChar::width(c).unwrap_or(0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::frame::SnapshotReason;
    use crate::vt::frame_ring::WireMessage;
    use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::Config;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc};

    fn make_term(cols: u16, rows: u16) -> Term<TermListener> {
        make_term_with_config(alacritty_terminal::term::Config::default(), cols, rows)
    }

    fn make_term_with_config(
        cfg: alacritty_terminal::term::Config,
        cols: u16,
        rows: u16,
    ) -> Term<TermListener> {
        let (reply_tx, _) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _) = mpsc::channel::<ControlFrame>(64);
        let _ = broadcast::channel::<WireMessage>(8);
        let listener = TermListener {
            reply_tx,
            control_tx,
            drop_counter: Arc::new(DropCounter::new()),
        };
        let size = crate::vt::bridge::test_dim(cols, rows);
        Term::new(cfg, &size, listener)
    }

    #[test]
    fn collect_full_on_fresh_term() {
        let mut term = make_term(10, 3);
        let mut scratch = Vec::new();
        // First damage query returns Full per alacritty contract.
        assert_eq!(collect_dirty_rows(&mut term, &mut scratch), DirtyRows::Full);
    }

    #[test]
    fn collect_partial_after_reset() {
        let mut term = make_term(10, 3);
        let mut scratch = Vec::new();
        let _ = collect_dirty_rows(&mut term, &mut scratch);
        term.reset_damage();
        // After reset with no input, alacritty returns Partial{cursor row}.
        match collect_dirty_rows(&mut term, &mut scratch) {
            DirtyRows::Full => panic!("expected Partial after reset"),
            DirtyRows::Rows(rows) => {
                // line_count is 1 (cursor blink dirties cursor row only).
                assert_eq!(rows.len(), 1);
            }
        }
    }

    fn install_text<T: alacritty_terminal::event::EventListener>(term: &mut Term<T>, text: &str) {
        let mut parser = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        parser.advance(term, text.as_bytes());
    }

    #[test]
    fn snapshot_empty_grid_yields_empty_or_space_rows() {
        let term = make_term(10, 3);
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 5, SnapshotReason::Initial, &mut interner, None);
        assert_eq!(snap.seq, 5);
        assert_eq!(snap.cols, 10);
        assert_eq!(snap.rows, 3);
        assert_eq!(snap.rows_data.len(), 3);
        for row in &snap.rows_data {
            assert!(
                row.runs.iter().all(|r| r.text.chars().all(|c| c == ' ')),
                "empty cells should serialize as space, not NUL; got runs={:?}",
                row.runs
            );
            // Each row must coalesce into exactly grid width's worth of cells.
            let total: u16 = row.runs.iter().map(|r| r.cols).sum();
            assert_eq!(total, snap.cols);
        }
    }

    #[test]
    fn snapshot_three_ascii_chars_one_run_prefix() {
        let mut term = make_term(10, 1);
        install_text(&mut term, "abc");
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        let row = &snap.rows_data[0];
        let merged: String = row.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(merged.starts_with("abc"), "got: {merged:?}");
    }

    #[test]
    fn snapshot_skips_wide_char_spacers() {
        let mut term = make_term(10, 1);
        // NOTE: "あ" is U+3042, East Asian Wide.
        install_text(&mut term, "あ");
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        let row = &snap.rows_data[0];
        let merged: String = row.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(merged.starts_with("あ"), "got: {merged:?}");
        // First run's cols must include the wide char's 2-column width.
        assert!(row.runs[0].cols >= 2);
    }

    #[test]
    fn snapshot_modes_includes_alt_screen_when_set() {
        let mut term = make_term(10, 1);
        install_text(&mut term, "\x1b[?1049h");
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        assert!(
            snap.modes.iter().any(|s| s == "alt-screen"),
            "expected alt-screen in modes; got {:?}",
            snap.modes
        );
    }

    #[test]
    fn snapshot_cursor_position_zero_zero_initially() {
        let term = make_term(10, 3);
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        assert_eq!(snap.cursor.x, 0);
        assert_eq!(snap.cursor.y, 0);
        assert!(snap.cursor.visible);
    }

    #[test]
    fn delta_single_dirty_row_yields_one_dirty_row() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "xyz");
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 9, &[0u16], &mut interner, None, vec![], vec![]);
        assert_eq!(delta.seq, 9);
        assert_eq!(delta.dirty_rows.len(), 1);
        assert_eq!(delta.dirty_rows[0].row, 0);
        let merged: String = delta.dirty_rows[0]
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert!(merged.starts_with("xyz"));
    }

    #[test]
    fn delta_multiple_dirty_rows_preserve_order() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "aaa\r\nbbb\r\nccc");
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 0, &[0, 2], &mut interner, None, vec![], vec![]);
        assert_eq!(delta.dirty_rows.len(), 2);
        assert_eq!(delta.dirty_rows[0].row, 0);
        assert_eq!(delta.dirty_rows[1].row, 2);
    }

    #[test]
    fn delta_empty_rows_slice_yields_empty_dirty_rows() {
        let term = make_term(10, 3);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 100, &[], &mut interner, None, vec![], vec![]);
        assert_eq!(delta.seq, 100);
        assert!(delta.dirty_rows.is_empty());
    }

    #[test]
    fn delta_carries_current_cursor_state() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "abc");
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 1, &[0], &mut interner, None, vec![], vec![]);
        assert_eq!(delta.cursor.x, 3);
        assert_eq!(delta.cursor.y, 0);
        assert!(delta.cursor.visible);
    }

    #[test]
    fn extract_cursor_reads_decscusr_blinking_underline() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "\x1b[3 q");
        let c = extract_cursor(&term);
        assert_eq!(c.shape, CursorShape::Underline);
        assert!(c.blinking, "shape 3 (blinking underline) → blinking=true");
    }

    #[test]
    fn extract_cursor_reads_steady_block() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "\x1b[2 q");
        let c = extract_cursor(&term);
        assert_eq!(c.shape, CursorShape::Block);
        assert!(!c.blinking, "shape 2 (steady block) → blinking=false");
    }

    #[test]
    fn extract_cursor_reads_blinking_bar() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "\x1b[5 q");
        let c = extract_cursor(&term);
        assert_eq!(c.shape, CursorShape::Bar);
        assert!(c.blinking);
    }

    #[test]
    fn extract_cursor_dectcem_hide() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "\x1b[?25l");
        let c = extract_cursor(&term);
        assert!(!c.visible, "DECTCEM hide (`?25l`) → visible=false");
    }

    #[test]
    fn extract_cursor_dectcem_show_after_hide() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "\x1b[?25l\x1b[?25h");
        let c = extract_cursor(&term);
        assert!(c.visible, "DECTCEM show after hide → visible=true");
    }

    #[test]
    fn build_snapshot_includes_display_offset_and_history_size() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 3);
        for _ in 0..5 {
            install_text(&mut term, "x\r\n");
        }
        term.scroll_display(Scroll::Delta(2));
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        assert_eq!(snap.display_offset, 2);
        assert!(snap.history_size >= 2, "history_size={}", snap.history_size);
    }

    #[test]
    fn viewport_row_to_line_at_zero_offset_is_identity() {
        let term = make_term(10, 24);
        for y in 0..24i32 {
            assert_eq!(viewport_row_to_line(&term, y), Line(y));
        }
    }

    #[test]
    fn viewport_row_to_line_with_offset_subtracts_display_offset() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 24);
        for _ in 0..30 {
            install_text(&mut term, "x\r\n");
        }
        term.scroll_display(Scroll::Delta(5));
        assert_eq!(term.grid().display_offset(), 5);
        assert_eq!(viewport_row_to_line(&term, 0), Line(-5));
        assert_eq!(viewport_row_to_line(&term, 5), Line(0));
        assert_eq!(viewport_row_to_line(&term, 23), Line(18));
    }

    #[test]
    fn viewport_row_to_line_at_max_offset_reaches_oldest_history() {
        let cfg = Config {
            scrolling_history: 50,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 4);
        for _ in 0..100 {
            install_text(&mut term, "x\r\n");
        }
        term.scroll_display(Scroll::Top);
        let off = term.grid().display_offset() as i32;
        assert!(off > 0, "expected non-zero display_offset, got {off}");
        // NOTE: y=0 must reach the oldest scrollback line when fully scrolled up.
        assert_eq!(viewport_row_to_line(&term, 0), Line(-off));
    }

    #[test]
    fn coalesce_row_reads_live_tail_when_not_scrolled() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "abc");
        let mut interner = HyperlinkInterner::new();
        let mut emitted = None;
        let runs = coalesce_row(&term, 0, &mut interner, &mut emitted);
        let merged: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert!(merged.starts_with("abc"), "got: {merged:?}");
    }

    #[test]
    fn coalesce_row_reads_scrollback_when_scrolled() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 3);
        // NOTE: push "marker" first so it ends up at the very top of history.
        install_text(&mut term, "marker\r\n");
        for i in 0..20 {
            install_text(&mut term, &format!("filler{i}\r\n"));
        }
        term.scroll_display(Scroll::Top);
        let mut interner = HyperlinkInterner::new();
        let mut emitted = None;
        let runs = coalesce_row(&term, 0, &mut interner, &mut emitted);
        let merged: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            merged.starts_with("marker"),
            "viewport row 0 should show oldest scrollback line; got {merged:?}",
        );
    }

    #[test]
    fn build_snapshot_after_scroll_contains_scrollback_content() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 3);
        install_text(&mut term, "alpha\r\n");
        for i in 0..20 {
            install_text(&mut term, &format!("noise{i}\r\n"));
        }
        term.scroll_display(Scroll::Top);
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        let row0: String = snap.rows_data[0]
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert!(
            row0.starts_with("alpha"),
            "snapshot row 0 after Scroll::Top should be oldest history line; got {row0:?}",
        );
    }

    #[test]
    fn build_delta_with_offset_reads_scrollback() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 3);
        install_text(&mut term, "alpha\r\n");
        for i in 0..20 {
            install_text(&mut term, &format!("noise{i}\r\n"));
        }
        term.scroll_display(Scroll::Top);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 0, &[0u16], &mut interner, None, vec![], vec![]);
        let row0: String = delta.dirty_rows[0]
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert!(
            row0.starts_with("alpha"),
            "delta row 0 after Scroll::Top should be oldest history line; got {row0:?}",
        );
    }

    #[test]
    fn extract_cursor_visible_at_zero_offset_uses_live_y() {
        let mut term = make_term(10, 5);
        install_text(&mut term, "\r\n\r\n");
        let c = extract_cursor(&term);
        assert_eq!(c.y, 2);
        assert!(c.visible);
    }

    #[test]
    fn extract_cursor_partial_scroll_keeps_visible_with_adjusted_y() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 5);
        install_text(&mut term, "x");
        for _ in 0..10 {
            install_text(&mut term, "\r\n");
        }
        term.scroll_display(Scroll::Delta(2));
        assert_eq!(term.grid().display_offset(), 2);
        let live_y = term.grid().cursor.point.line.0;
        let c = extract_cursor(&term);
        if live_y + 2 < 5 {
            assert_eq!(c.y, (live_y + 2) as u16);
            assert!(c.visible);
        } else {
            assert!(
                !c.visible,
                "live_y={live_y} + 2 should push cursor off viewport"
            );
        }
    }

    #[test]
    fn extract_cursor_hidden_when_scrolled_past_live_grid() {
        let cfg = Config {
            scrolling_history: 200,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 5);
        for _ in 0..50 {
            install_text(&mut term, "x\r\n");
        }
        term.scroll_display(Scroll::Top);
        let c = extract_cursor(&term);
        assert!(
            !c.visible,
            "cursor should be hidden when display_offset >= screen_lines"
        );
    }

    #[test]
    fn build_snapshot_sets_produced_at_us_when_provided() {
        let term = make_term(2, 2);
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, Some(99));
        assert_eq!(snap.produced_at_us, Some(99));
    }

    #[test]
    fn build_snapshot_leaves_produced_at_us_none_by_default() {
        let term = make_term(2, 2);
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial, &mut interner, None);
        assert!(snap.produced_at_us.is_none());
    }

    #[test]
    fn build_delta_sets_produced_at_us_when_provided() {
        let term = make_term(2, 2);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 0, &[0u16], &mut interner, Some(123), vec![], vec![]);
        assert_eq!(delta.produced_at_us, Some(123));
    }

    #[test]
    fn build_delta_carries_inline_modes() {
        let term = make_term(2, 2);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(
            &term,
            7,
            &[0u16],
            &mut interner,
            None,
            vec!["bracketed-paste".to_string()],
            vec!["alt-screen".to_string()],
        );
        assert_eq!(delta.seq, 7);
        assert_eq!(delta.modes_added, vec!["bracketed-paste".to_string()]);
        assert_eq!(delta.modes_removed, vec!["alt-screen".to_string()]);
    }

    #[test]
    fn build_delta_defaults_empty_modes() {
        let term = make_term(2, 2);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, 0, &[0u16], &mut interner, None, vec![], vec![]);
        assert!(delta.modes_added.is_empty());
        assert!(delta.modes_removed.is_empty());
    }
}
