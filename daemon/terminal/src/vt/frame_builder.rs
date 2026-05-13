//! Pure functions that turn a Term snapshot into wire frames.
//!
//! All entry points take the Term by mutable reference because
//! `Term::damage()` mutates internal damage state. Callers must hold the
//! `vt_state` lock; this module performs no locking.

use crate::vt::frame::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, ModeFrame, Row, Run,
    SnapshotReason, style,
};
use crate::vt::mode_diff::{TRACKED_MODES, diff_mode};
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
pub fn collect_dirty_rows<T>(term: &mut Term<T>) -> DirtyRows {
    match term.damage() {
        TermDamage::Full => DirtyRows::Full,
        TermDamage::Partial(iter) => DirtyRows::Rows(iter.map(|d| d.line as u16).collect()),
    }
}

/// Computes a `ModeFrame` from a `TermMode` transition. Returns `None` when
/// no tracked flag changed.
pub fn build_mode(prev: TermMode, curr: TermMode, seq: u32) -> Option<ModeFrame> {
    let change = diff_mode(prev, curr);
    if change.is_empty() {
        return None;
    }
    Some(ModeFrame::new(
        seq,
        change.added.into_iter().map(String::from).collect(),
        change.removed.into_iter().map(String::from).collect(),
    ))
}

/// Builds a full-screen snapshot frame.
///
/// Reads the current terminal grid via shared reference, coalescing each row
/// into runs of cells with identical attributes. Wide-char spacer cells are
/// skipped so the wide character itself accounts for its full column width.
pub fn build_snapshot<T>(term: &Term<T>, seq: u32, reason: SnapshotReason) -> FrameSnapshot {
    let cols = term.columns() as u16;
    let rows = term.screen_lines() as u16;
    let rows_data: Vec<Row> = (0..rows as i32)
        .map(|y| Row {
            runs: coalesce_row(term, y),
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
    }
}

/// Builds a delta frame containing only the listed dirty rows.
///
/// Each entry is a full-row replacement (not partial). Row ordering follows
/// the supplied slice.
pub fn build_delta<T>(term: &Term<T>, seq: u32, rows: &[u16]) -> FrameDelta {
    let dirty_rows: Vec<DirtyRow> = rows
        .iter()
        .map(|&r| DirtyRow {
            row: r,
            runs: coalesce_row(term, r as i32),
        })
        .collect();
    FrameDelta { seq, dirty_rows }
}

fn extract_cursor<T>(term: &Term<T>) -> Cursor {
    let point = term.grid().cursor.point;
    let mut x = point.column.0 as u16;
    let y = point.line.0 as u16;
    // NOTE: alacritty's RenderableCursor shifts x left by 1 when the cursor
    // lands on a wide-char spacer; replicate so the visible cursor aligns
    // with the wide glyph itself.
    let cell = &term.grid()[Line(point.line.0)][point.column];
    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && x > 0 {
        x -= 1;
    }
    Cursor {
        x,
        y,
        shape: CursorShape::Block,
        visible: term.mode().contains(TermMode::SHOW_CURSOR),
    }
}

fn snapshot_modes(curr: TermMode) -> Vec<String> {
    TRACKED_MODES
        .iter()
        .filter(|(flag, _)| curr.contains(*flag))
        .map(|(_, name)| (*name).to_string())
        .collect()
}

fn coalesce_row<T>(term: &Term<T>, y: i32) -> Vec<Run> {
    let cols = term.columns() as u16;
    let grid_row = &term.grid()[Line(y)];
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
        let cell_attrs = RunAttrs::from_cell(cell);
        let cell_width = char_width(cell.c);
        match &acc_attrs {
            Some(prev) if *prev == cell_attrs => {
                acc_text.push(cell.c);
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
                acc_text.push(cell.c);
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
}

impl RunAttrs {
    fn from_cell(cell: &Cell) -> Self {
        Self {
            fg: color_from_alacritty(cell.fg),
            bg: color_from_alacritty(cell.bg),
            style: style_from_flags(cell.flags),
        }
    }

    fn into_run(self, text: String, cols: u16) -> Run {
        Run {
            cols,
            fg: self.fg,
            bg: self.bg,
            style: self.style,
            text,
            hyperlink_id: None,
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
    use alacritty_terminal::term::TermMode;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc};

    fn make_term(cols: u16, rows: u16) -> Term<TermListener> {
        let (reply_tx, _) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _) = mpsc::channel::<ControlFrame>(64);
        let _ = broadcast::channel::<WireMessage>(8);
        let listener = TermListener {
            reply_tx,
            control_tx,
            drop_counter: Arc::new(DropCounter::new()),
        };
        let size = crate::vt::bridge::test_dim(cols, rows);
        Term::new(alacritty_terminal::term::Config::default(), &size, listener)
    }

    #[test]
    fn collect_full_on_fresh_term() {
        let mut term = make_term(10, 3);
        // First damage query returns Full per alacritty contract.
        assert_eq!(collect_dirty_rows(&mut term), DirtyRows::Full);
    }

    #[test]
    fn build_mode_enter_alt_screen() {
        let prev = TermMode::empty();
        let curr = TermMode::ALT_SCREEN;
        let m = build_mode(prev, curr, 42).expect("transition present");
        assert_eq!(m.seq, 42);
        assert_eq!(m.added, vec!["alt-screen".to_string()]);
        assert!(m.removed.is_empty());
    }

    #[test]
    fn build_mode_no_change_returns_none() {
        let m = TermMode::BRACKETED_PASTE;
        assert!(build_mode(m, m, 1).is_none());
    }

    #[test]
    fn collect_partial_after_reset() {
        let mut term = make_term(10, 3);
        let _ = collect_dirty_rows(&mut term);
        term.reset_damage();
        // After reset with no input, alacritty returns Partial{cursor row}.
        match collect_dirty_rows(&mut term) {
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
        let snap = build_snapshot(&term, 5, SnapshotReason::Initial);
        assert_eq!(snap.seq, 5);
        assert_eq!(snap.cols, 10);
        assert_eq!(snap.rows, 3);
        assert_eq!(snap.rows_data.len(), 3);
        for row in &snap.rows_data {
            assert!(
                row.runs
                    .iter()
                    .all(|r| r.text.chars().all(|c| c == ' ' || c == '\0'))
            );
        }
    }

    #[test]
    fn snapshot_three_ascii_chars_one_run_prefix() {
        let mut term = make_term(10, 1);
        install_text(&mut term, "abc");
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial);
        let row = &snap.rows_data[0];
        let merged: String = row.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(merged.starts_with("abc"), "got: {merged:?}");
    }

    #[test]
    fn snapshot_skips_wide_char_spacers() {
        let mut term = make_term(10, 1);
        // NOTE: "あ" is U+3042, East Asian Wide.
        install_text(&mut term, "あ");
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial);
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
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial);
        assert!(
            snap.modes.iter().any(|s| s == "alt-screen"),
            "expected alt-screen in modes; got {:?}",
            snap.modes
        );
    }

    #[test]
    fn snapshot_cursor_position_zero_zero_initially() {
        let term = make_term(10, 3);
        let snap = build_snapshot(&term, 0, SnapshotReason::Initial);
        assert_eq!(snap.cursor.x, 0);
        assert_eq!(snap.cursor.y, 0);
        assert!(snap.cursor.visible);
    }

    #[test]
    fn delta_single_dirty_row_yields_one_dirty_row() {
        let mut term = make_term(10, 3);
        install_text(&mut term, "xyz");
        let delta = build_delta(&term, 9, &[0u16]);
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
        let delta = build_delta(&term, 0, &[0, 2]);
        assert_eq!(delta.dirty_rows.len(), 2);
        assert_eq!(delta.dirty_rows[0].row, 0);
        assert_eq!(delta.dirty_rows[1].row, 2);
    }

    #[test]
    fn delta_empty_rows_slice_yields_empty_dirty_rows() {
        let term = make_term(10, 3);
        let delta = build_delta(&term, 100, &[]);
        assert_eq!(delta.seq, 100);
        assert!(delta.dirty_rows.is_empty());
    }
}
