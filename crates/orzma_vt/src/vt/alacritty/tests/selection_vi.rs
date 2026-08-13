//! Vi-cursor anchoring and selection-granularity tests.

use super::*;

fn enter_vi_at(vt: &mut AlacrittyVtBackend, x: usize, y: i32) {
    vt.term.toggle_vi_mode();
    vt.term.vi_mode_cursor.point = AlacPoint::new(Line(y), Column(x));
}

/// Asserts that `StartAtViCursor` anchors at the vi cursor the VT
/// tracks internally.
///
/// Case: the user presses `v` in vi mode.
#[test]
fn start_at_vi_cursor_anchors_at_the_vi_cursor() {
    let mut vt = vt_after(b"hello");
    enter_vi_at(&mut vt, 2, 0);
    vt.apply_selection(SelectionOp::StartAtViCursor {
        kind: SelectionKind::Simple,
    })
    .unwrap();
    assert_eq!(vt.selected_text().as_deref(), Some("l"));
}

/// Asserts that `ChangeKind` switches granularity while preserving
/// the original anchor, spanning to the current vi cursor.
///
/// Case: the user presses `v`, moves the vi cursor, then presses `V`
/// without leaving vi mode. The decided policy preserves the original
/// anchor rather than re-anchoring at the vi cursor.
#[test]
fn change_kind_switches_granularity_and_keeps_the_anchor() {
    let mut vt = vt_after(b"abcdefghij\r\nklmnopqrst");
    enter_vi_at(&mut vt, 2, 0);
    vt.apply_selection(SelectionOp::StartAtViCursor {
        kind: SelectionKind::Simple,
    })
    .unwrap();
    vt.term.vi_mode_cursor.point = AlacPoint::new(Line(1), Column(7));
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(
        vt.selected_text().as_deref(),
        Some("abcdefghij\nklmnopqrst\n"),
        "Lines spanning the preserved row-0 anchor through the vi cursor on row 1"
    );
}

/// Asserts that `ChangeKind` with no active selection changes
/// nothing.
///
/// Case: the selection vanishes between the host's kind read and the
/// applied toggle.
#[test]
fn change_kind_without_a_selection_is_a_no_op() {
    let mut vt = vt_after(b"abc");
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_range(), None);
}

/// Asserts that a `Lines` start selects the logical row: full-width
/// range, `Lines` geometry, and content-plus-newline text.
///
/// Case: the user presses vi `V` (or triple-clicks) on a line. The
/// decided policy extracts the populated row plus a trailing `\n`,
/// not an 80-column space-padded row.
#[test]
fn lines_kind_selects_the_logical_row_with_trailing_newline() {
    let mut vt = vt_after(b"hello world");
    vt.apply_selection(SelectionOp::StartAt {
        cell: cell(3, 0),
        side: CellSide::Left,
        kind: SelectionKind::Lines,
    })
    .unwrap();
    let range = vt.selection_range().expect("Lines start must render");
    assert_eq!(range.start, ViewportPoint { row: 0, column: 0 });
    assert_eq!(range.end, ViewportPoint { row: 0, column: 79 });
    assert_eq!(range.geometry, SelectionGeometry::Lines);
    assert_eq!(vt.selected_text().as_deref(), Some("hello world\n"));
}

/// Asserts that `switch_vi_mode` toggles the mode on a real
/// transition and reports full damage.
///
/// Case: the user enters and leaves vi mode; the vi cursor overlay
/// appears and disappears without any PTY output.
#[test]
fn switch_vi_mode_transitions_and_reports_full_damage() {
    let mut vt = vt_after(b"hello");
    assert_eq!(
        vt.switch_vi_mode(ViModeSwitch::Enter).unwrap(),
        Some(Damage::Full)
    );
    assert!(vt.term.mode().contains(TermMode::VI));
    assert_eq!(
        vt.switch_vi_mode(ViModeSwitch::Exit).unwrap(),
        Some(Damage::Full)
    );
    assert!(!vt.term.mode().contains(TermMode::VI));
}

/// Asserts that an idempotent vi-mode request changes nothing and
/// reports no damage.
///
/// Case: a repeated Enter arrives, or an Exit while the terminal is
/// already in normal mode.
#[test]
fn an_idempotent_vi_mode_request_reports_no_damage() {
    let mut vt = vt_after(b"hello");
    assert_eq!(vt.switch_vi_mode(ViModeSwitch::Exit).unwrap(), None);
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.switch_vi_mode(ViModeSwitch::Enter).unwrap(), None);
    assert!(vt.term.mode().contains(TermMode::VI));
}

/// Asserts that `selection_kind` tracks the active granularity and
/// resets on `Clear`.
///
/// Case: the user toggles selection granularity with `v` and `V` in
/// vi mode.
#[test]
fn selection_kind_reports_the_active_granularity_and_resets() {
    let mut vt = vt_after(b"abcdef");
    assert_eq!(vt.selection_kind(), None);
    start_simple(&mut vt, 0, 0);
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Simple));
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_kind(), Some(SelectionKind::Lines));
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.selection_kind(), None);
}
