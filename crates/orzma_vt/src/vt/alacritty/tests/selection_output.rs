//! Selection projection, invalidation, getter, text-extraction, and
//! selection-damage tests.

use super::*;

/// Asserts that a negative grid line addresses a scrollback row
/// without any display-offset dependency.
///
/// Case: the host UI has already resolved a click made while scrolled
/// back into the grid cell it names, and the VT applies that cell the
/// same way regardless of where the viewport sits.
#[test]
fn a_negative_line_addresses_a_history_row() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    start_simple(&mut vt, 0, -3);
    update_to(&mut vt, 1, -3, CellSide::Right);
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
    assert_eq!(
        vt.modes().active_screen,
        ScreenKind::Alternate,
        "precondition: alt screen entered"
    );
    assert_eq!(vt.selection_range(), None);
    assert_eq!(vt.selection_kind(), None);
    vt.change_selection_kind(SelectionKind::Lines).unwrap();
    assert_eq!(vt.selection_range(), None, "no rebuild from a stale anchor");
}

/// Asserts that every operation which changes the visible selection
/// reports full damage: start_selection, a moving update_selection,
/// change_selection_kind, and clear_selection.
///
/// Case: the user presses, drags, switches granularity with v/V, and
/// clears, all on an otherwise idle terminal.
#[test]
fn every_visible_selection_change_reports_full_damage() {
    let mut vt = vt_after(b"abcdefghij");
    assert_eq!(
        vt.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple)
            .unwrap(),
        Some(StagedDamage::Full),
        "start_selection"
    );
    assert_eq!(
        vt.update_selection(point(0, 4), CellSide::Right).unwrap(),
        Some(StagedDamage::Full),
        "update_selection"
    );
    assert_eq!(
        vt.change_selection_kind(SelectionKind::Lines).unwrap(),
        Some(StagedDamage::Full),
        "change_selection_kind"
    );
    assert_eq!(
        vt.clear_selection().unwrap(),
        Some(StagedDamage::Full),
        "clear_selection"
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
        vt.update_selection(point(0, 2), CellSide::Right).unwrap(),
        None
    );
    assert_eq!(
        vt.change_selection_kind(SelectionKind::Lines).unwrap(),
        None
    );
    assert_eq!(vt.clear_selection().unwrap(), None);
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

/// Asserts that the reported selection range is invariant under
/// display scrolling.
///
/// Case: the user selects a span and then scrolls back and forth
/// through history; the selection stays on the text it covered, so
/// the reported grid range never moves.
#[test]
fn the_selection_range_is_invariant_under_display_scroll() {
    let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
    start_simple(&mut vt, 0, 0);
    update_to(&mut vt, 5, 15, CellSide::Right);
    let before = vt.selection_range().expect("selection renders");
    assert_eq!((before.start, before.end), (point(0, 0), point(15, 5)));
    vt.scroll(Scroll::Delta(10));
    assert_eq!(vt.selection_range(), Some(before));
    vt.scroll(Scroll::Bottom);
    assert_eq!(vt.selection_range(), Some(before));
}

/// Asserts that `selected_text` is `None` on a fresh VT and after a
/// `clear_selection`.
///
/// Case: the user copies with nothing selected, and again right
/// after clearing a selection.
#[test]
fn selected_text_is_none_without_a_selection() {
    let mut vt = vt_after(b"abc");
    assert_eq!(vt.selected_text(), None);
    start_simple(&mut vt, 0, 0);
    vt.clear_selection().unwrap();
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
