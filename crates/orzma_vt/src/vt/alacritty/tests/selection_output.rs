//! Selection projection, invalidation, getter, text-extraction, and
//! selection-damage tests.

use super::*;

/// Asserts that `StartAt` resolves viewport rows through the display
/// offset onto the scrollback rows the user actually sees.
///
/// Case: the user selects text while scrolled back into history.
#[test]
fn start_at_translates_viewport_rows_through_display_offset() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(3));
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 1, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("l7"));
}

/// Asserts that an alt-screen swap invalidates the selection, its
/// reported kind, and the stored anchor.
///
/// Case: the user opens vim mid-selection.
#[test]
fn an_alt_screen_swap_invalidates_selection_and_anchor() {
    let mut vt = vt_after(b"abcdef");
    start_simple(&mut vt, 0, 0);
    vt.interpret(b"\x1b[?1049h");
    assert!(vt.modes().alt_screen, "precondition: alt screen entered");
    assert_eq!(vt.selection_range(), None);
    assert_eq!(vt.selection_kind(), None);
    vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
        .unwrap();
    assert_eq!(vt.selection_range(), None, "no rebuild from a stale anchor");
}

/// Asserts that every operation which changes the visible selection
/// reports full damage: StartAt, a moving UpdateTo, ChangeKind, and
/// Clear.
///
/// Case: the user presses, drags, switches granularity with v/V, and
/// clears, all on an otherwise idle terminal.
#[test]
fn every_visible_selection_change_reports_full_damage() {
    let mut vt = vt_after(b"abcdefghij");
    assert_eq!(
        vt.apply_selection(SelectionOp::StartAt {
            cell: cell(0, 0),
            side: CellSide::Left,
            kind: SelectionKind::Simple,
        })
        .unwrap(),
        Some(Damage::Full),
        "StartAt"
    );
    assert_eq!(
        vt.apply_selection(SelectionOp::UpdateTo {
            cell: cell(4, 0),
            side: CellSide::Right,
        })
        .unwrap(),
        Some(Damage::Full),
        "UpdateTo"
    );
    assert_eq!(
        vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
            .unwrap(),
        Some(Damage::Full),
        "ChangeKind"
    );
    assert_eq!(
        vt.apply_selection(SelectionOp::Clear).unwrap(),
        Some(Damage::Full),
        "Clear"
    );
}

/// Asserts that selection operations with no active selection report
/// no damage.
///
/// Case: an alt-screen swap wipes the selection while the input glue
/// still delivers one more drag, kind switch, or clear.
#[test]
fn no_op_selection_ops_report_no_damage() {
    let mut vt = vt_after(b"abc");
    assert_eq!(
        vt.apply_selection(SelectionOp::UpdateTo {
            cell: cell(2, 0),
            side: CellSide::Right,
        })
        .unwrap(),
        None
    );
    assert_eq!(
        vt.apply_selection(SelectionOp::ChangeKind(SelectionKind::Lines))
            .unwrap(),
        None
    );
    assert_eq!(vt.apply_selection(SelectionOp::Clear).unwrap(), None);
}

/// Asserts that a fresh VT reports no selection range.
///
/// Case: the renderer reads the selection overlay on a freshly
/// spawned terminal.
#[test]
fn selection_range_is_none_on_a_fresh_vt() {
    assert_eq!(
        AlacrittyVtBackend::new(80, GRID_ROWS).selection_range(),
        None
    );
}

/// Asserts that display scrolling shifts the projected viewport rows
/// and clamps off-viewport endpoints to the -1 / row-count
/// sentinels.
///
/// Case: the user selects a span and then scrolls it partly out of
/// the viewport.
#[test]
fn display_scroll_shifts_and_clamps_the_projected_range() {
    let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 5, 15, CellSide::Right);
    vt.scroll(Scroll::Delta(3));
    let range = vt.selection_range().expect("selection survives scroll");
    assert_eq!((range.start.row, range.end.row), (3, 18), "pure shift");
    vt.scroll(Scroll::Delta(7));
    let range = vt.selection_range().expect("still partially visible");
    assert_eq!(range.start.row, 10);
    assert_eq!(range.end.row, GRID_ROWS as i16, "below-viewport sentinel");
    vt.apply_selection(SelectionOp::Clear).unwrap();
    start_simple(&mut vt, 0, 5);
    update_to(&mut vt, 5, 20, CellSide::Right);
    vt.scroll(Scroll::Bottom);
    let range = vt.selection_range().expect("still partially visible");
    assert_eq!(range.start.row, -1, "above-viewport sentinel");
    assert_eq!(range.end.row, 10);
}

/// Asserts that `selected_text` is `None` on a fresh VT and after a
/// `Clear`.
///
/// Case: the user copies with nothing selected, and again right
/// after clearing a selection.
#[test]
fn selected_text_is_none_without_a_selection() {
    let mut vt = vt_after(b"abc");
    assert_eq!(vt.selected_text(), None);
    start_simple(&mut vt, 0, 0);
    vt.apply_selection(SelectionOp::Clear).unwrap();
    assert_eq!(vt.selected_text(), None);
}

/// Asserts that `selected_text` extracts wide characters exactly
/// once, joins soft-wrapped rows without a newline, and keeps hard
/// line breaks.
///
/// Case: the user copies CJK output and long soft-wrapped command
/// lines.
#[test]
fn selected_text_handles_wide_and_wrapped_content() {
    let mut vt = vt_after("あ".as_bytes());
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 1, 0, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("あ"));

    let long = "x".repeat(85);
    let mut vt = vt_after(long.as_bytes());
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 4, 1, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some(long.as_str()));

    let mut vt = vt_after(b"ab\r\ncd");
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 1, 1, CellSide::Right);
    assert_eq!(vt.selected_text().as_deref(), Some("ab\ncd"));
}
