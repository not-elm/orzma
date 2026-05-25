//! Pure functions that turn a Term snapshot into wire frames.
//!
//! All entry points take the Term by mutable reference because
//! `Term::damage()` mutates internal damage state. Callers must hold the
//! `vt_state` lock; this module performs no locking.

use crate::palette::acolor_to_bevy;
use crate::vt::hyperlink::HyperlinkInterner;
use crate::vt::mode_diff::TRACKED_MODES;
use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use bevy::ecs::entity::Entity;
use bevy::prelude::Color;
use bevy_terminal_renderer::prelude::{
    Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Hyperlink, HyperlinkId, HyperlinkUri,
    Row, Run, SelectionKind, SelectionRange, SnapshotReason, ViCursor, ViewportPoint,
};
use unicode_width::UnicodeWidthChar;

/// Style bitmask constants (kept compatible with the schema's
/// `Run.style: u16`).
pub(crate) mod style {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 2;
    pub const UNDERLINE: u16 = 4;
    pub const STRIKE: u16 = 8;
    pub const REVERSE: u16 = 16;
    pub const DIM: u16 = 32;
}

/// Builds a full-screen snapshot frame.
///
/// Reads the current terminal grid via shared reference, coalescing each row
/// into runs of cells with identical attributes. Wide-char spacer cells are
/// skipped so the wide character itself accounts for its full column width.
pub(crate) fn build_snapshot<T>(
    term: &Term<T>,
    entity: Entity,
    seq: u32,
    reason: SnapshotReason,
    interner: &mut HyperlinkInterner,
) -> FrameSnapshot {
    let cols = term.columns() as u16;
    let rows = term.screen_lines() as u16;
    let mut hyperlinks_opt: Option<Vec<(HyperlinkId, HyperlinkUri)>> = None;
    let rows_data: Vec<Row> = (0..rows as i32)
        .map(|y| Row {
            runs: coalesce_row(term, y, interner, &mut hyperlinks_opt),
        })
        .collect();
    FrameSnapshot {
        entity,
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
        vi_cursor: extract_vi_cursor(term),
        selection: extract_selection_range(term),
    }
}

/// Builds a delta frame containing only the listed dirty rows.
///
/// Each entry is a full-row replacement (not partial). Row ordering follows
/// the supplied slice.
pub(crate) fn build_delta<T>(
    term: &Term<T>,
    entity: Entity,
    seq: u32,
    rows: &[u16],
    interner: &mut HyperlinkInterner,
) -> FrameDelta {
    let mut hyperlinks_opt: Option<Vec<(HyperlinkId, HyperlinkUri)>> = None;
    let dirty_rows: Vec<DirtyRow> = rows
        .iter()
        .map(|&r| DirtyRow {
            row: r,
            runs: coalesce_row(term, r as i32, interner, &mut hyperlinks_opt),
        })
        .collect();
    FrameDelta {
        entity,
        seq,
        cursor: extract_cursor(term),
        dirty_rows,
        hyperlinks: hyperlinks_opt
            .unwrap_or_default()
            .into_iter()
            .map(|(id, uri)| Hyperlink { id, uri })
            .collect(),
        display_offset: term.grid().display_offset() as u32,
        vi_cursor: extract_vi_cursor(term),
        selection: extract_selection_range(term),
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

/// Builds a wire `ViCursor` from alacritty's vi-mode cursor.
///
/// Returns `None` when alacritty is NOT in vi mode (`TermMode::VI`
/// unset). Otherwise translates `Term::vi_mode_cursor.point.line`
/// (live-grid coords, may be negative for scrollback) into viewport
/// coordinates via the current `display_offset`. When the resulting
/// viewport row falls above the visible area, `in_scrollback` is
/// set and `row` is clamped to `-1` per the schema convention at
/// `crates/bevy_terminal_renderer/src/schema/cursor.rs:12`.
pub(crate) fn extract_vi_cursor<T>(term: &Term<T>) -> Option<ViCursor> {
    if !term.mode().contains(TermMode::VI) {
        return None;
    }
    let p = term.vi_mode_cursor.point;
    let off = term.grid().display_offset() as i32;
    let screen_lines = term.screen_lines() as i32;
    let viewport_row = p.line.0 + off;
    if viewport_row < 0 {
        return Some(ViCursor {
            row: -1,
            column: p.column.0 as u16,
            in_scrollback: true,
        });
    }
    let max_row = screen_lines.saturating_sub(1);
    let row_clamped = viewport_row.min(max_row);
    Some(ViCursor {
        row: row_clamped as i16,
        column: p.column.0 as u16,
        in_scrollback: false,
    })
}

/// Builds a wire `SelectionRange` from `term.selection`. Returns
/// `None` when no selection is active or when the selection range is
/// empty (alacritty's `Selection::to_range` returns `None` in that
/// case; see `alacritty_terminal/selection.rs:332`).
pub(crate) fn extract_selection_range<T>(term: &Term<T>) -> Option<SelectionRange> {
    let sel = term.selection.as_ref()?;
    let range = sel.to_range(term)?;
    let kind = match sel.ty {
        SelectionType::Lines => SelectionKind::Line,
        SelectionType::Simple | SelectionType::Block | SelectionType::Semantic => {
            SelectionKind::Char
        }
    };
    Some(SelectionRange {
        start: viewport_point_of(term, range.start),
        end: viewport_point_of(term, range.end),
        kind,
    })
}

/// Maps an alacritty `Point` (live-grid Line; usize Column) into a
/// wire `ViewportPoint`. Rows that resolve above the viewport are
/// clamped to `-1`; rows that resolve below are clamped to `rows`.
/// Caller is responsible for the column-bounds invariant (alacritty
/// already keeps `point.column` in `[0, cols)` for vi cursor + selection
/// endpoints, so a tighter assert is unnecessary).
fn viewport_point_of<T>(term: &Term<T>, p: Point) -> ViewportPoint {
    let off = term.grid().display_offset() as i32;
    let screen_lines = term.screen_lines() as i32;
    let viewport_row = p.line.0 + off;
    let clamped = viewport_row.clamp(-1, screen_lines);
    ViewportPoint {
        row: clamped as i16,
        column: p.column.0 as u16,
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
pub(crate) fn viewport_row_to_line<T>(term: &Term<T>, y: i32) -> Line {
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
    emitted_hyperlinks: &mut Option<Vec<(HyperlinkId, HyperlinkUri)>>,
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
        // Resolve hyperlink (alac String id → HyperlinkId via interner).
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

#[derive(Debug, Clone, PartialEq)]
struct RunAttrs {
    fg: Color,
    bg: Color,
    style: u16,
    hyperlink_id: Option<HyperlinkId>,
}

impl RunAttrs {
    fn from_cell(cell: &Cell, hyperlink_id: Option<HyperlinkId>) -> Self {
        Self {
            fg: acolor_to_bevy(cell.fg),
            bg: acolor_to_bevy(cell.bg),
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

fn style_from_flags(flags: Flags) -> u16 {
    let mut s: u16 = 0;
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
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::Config;
    use crossbeam_channel::unbounded;

    fn make_term(cols: u16, rows: u16) -> Term<crate::vt::listener::TermListener> {
        make_term_with_config(Config::default(), cols, rows)
    }

    fn make_term_with_config(
        cfg: Config,
        cols: u16,
        rows: u16,
    ) -> Term<crate::vt::listener::TermListener> {
        let (reply_tx, _) = unbounded::<Vec<u8>>();
        let (control_tx, _) = unbounded::<crate::vt::listener::ControlFrame>();
        let listener = crate::vt::listener::TermListener {
            reply_tx,
            control_tx,
        };
        let size = LocalDim {
            cols: cols.into(),
            rows: rows.into(),
        };
        Term::new(cfg, &size, listener)
    }

    struct LocalDim {
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            5,
            SnapshotReason::Initial,
            &mut interner,
        );
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
        assert_eq!(snap.cursor.x, 0);
        assert_eq!(snap.cursor.y, 0);
        assert!(snap.cursor.visible);
    }

    #[test]
    fn delta_single_dirty_row_yields_one_dirty_row() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "xyz");
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, Entity::PLACEHOLDER, 9, &[0u16], &mut interner);
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
        let delta = build_delta(&term, Entity::PLACEHOLDER, 0, &[0, 2], &mut interner);
        assert_eq!(delta.dirty_rows.len(), 2);
        assert_eq!(delta.dirty_rows[0].row, 0);
        assert_eq!(delta.dirty_rows[1].row, 2);
    }

    #[test]
    fn delta_empty_rows_slice_yields_empty_dirty_rows() {
        let term = make_term(10, 3);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, Entity::PLACEHOLDER, 100, &[], &mut interner);
        assert_eq!(delta.seq, 100);
        assert!(delta.dirty_rows.is_empty());
    }

    #[test]
    fn delta_carries_current_cursor_state() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "abc");
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, Entity::PLACEHOLDER, 1, &[0], &mut interner);
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
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
        let mut emitted: Option<Vec<(HyperlinkId, HyperlinkUri)>> = None;
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
        let mut emitted: Option<Vec<(HyperlinkId, HyperlinkUri)>> = None;
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
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
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
        let delta = build_delta(&term, Entity::PLACEHOLDER, 0, &[0u16], &mut interner);
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
    fn extract_vi_cursor_none_when_not_in_vi_mode() {
        let term = make_term(10, 5);
        assert!(
            extract_vi_cursor(&term).is_none(),
            "vi cursor must be None when TermMode::VI is unset"
        );
    }

    #[test]
    fn extract_vi_cursor_returns_in_scrollback_true_for_negative_line() {
        let cfg = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = make_term_with_config(cfg, 10, 5);
        for _ in 0..20 {
            install_text(&mut term, "x\r\n");
        }
        term.toggle_vi_mode();
        assert_eq!(
            term.grid().display_offset(),
            0,
            "test precondition: display_offset must be 0 so the negative-line clamp is the only path to in_scrollback",
        );
        term.vi_mode_cursor.point = alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(-10),
            alacritty_terminal::index::Column(3),
        );
        let vc = extract_vi_cursor(&term).expect("vi cursor present in vi mode");
        assert!(
            vc.in_scrollback,
            "negative line + zero display_offset must report in_scrollback"
        );
        assert_eq!(vc.row, -1, "in_scrollback must clamp row to -1");
        assert_eq!(vc.column, 3);
    }

    #[test]
    fn extract_vi_cursor_clamps_row_to_last_visible_row_not_one_past() {
        let mut term = make_term(10, 5);
        term.toggle_vi_mode();
        // Force vi cursor to a position that maps to viewport_row == screen_lines.
        // With display_offset = 0 and screen_lines = 5, line == 5 maps to row 5
        // (one past the last valid row).
        term.vi_mode_cursor.point = alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(5),
            alacritty_terminal::index::Column(2),
        );
        let vc = extract_vi_cursor(&term).expect("vi mode on");
        assert!(
            vc.row < 5,
            "vi cursor row must be < screen_lines (5), got {}",
            vc.row,
        );
        assert_eq!(
            vc.row, 4,
            "vi cursor row must clamp to last visible row (screen_lines - 1)",
        );
    }

    #[test]
    fn extract_selection_range_returns_none_when_term_selection_is_none() {
        let term = make_term(10, 5);
        assert!(extract_selection_range(&term).is_none());
    }

    #[test]
    fn extract_selection_range_kind_lines_when_selection_is_lines() {
        use alacritty_terminal::index::Side;
        use alacritty_terminal::selection::Selection;
        let mut term = make_term(10, 5);
        install_text(&mut term, "abc\r\ndef\r\nghi");
        let start = alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(0),
            alacritty_terminal::index::Column(0),
        );
        let end = alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(1),
            alacritty_terminal::index::Column(2),
        );
        let mut sel = Selection::new(SelectionType::Lines, start, Side::Left);
        sel.update(end, Side::Right);
        term.selection = Some(sel);
        let range = extract_selection_range(&term).expect("selection present");
        assert_eq!(range.kind, SelectionKind::Line);
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
    fn build_snapshot_carries_vi_cursor_when_in_vi_mode() {
        let mut term = make_term(10, 5);
        install_text(&mut term, "abc");
        term.toggle_vi_mode();
        let mut interner = HyperlinkInterner::new();
        let snap = build_snapshot(
            &term,
            Entity::PLACEHOLDER,
            0,
            SnapshotReason::Initial,
            &mut interner,
        );
        assert!(
            snap.vi_cursor.is_some(),
            "vi mode on → snapshot must carry vi_cursor"
        );
    }

    #[test]
    fn build_delta_carries_selection_range_when_selection_present() {
        use alacritty_terminal::index::Side;
        use alacritty_terminal::selection::Selection;
        let mut term = make_term(10, 3);
        install_text(&mut term, "abc");
        let p = alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(0),
            alacritty_terminal::index::Column(0),
        );
        let mut sel = Selection::new(SelectionType::Simple, p, Side::Left);
        sel.update(p, Side::Right);
        term.selection = Some(sel);
        let mut interner = HyperlinkInterner::new();
        let delta = build_delta(&term, Entity::PLACEHOLDER, 0, &[0u16], &mut interner);
        assert!(
            delta.selection.is_some(),
            "selection present → delta must carry it"
        );
    }
}
