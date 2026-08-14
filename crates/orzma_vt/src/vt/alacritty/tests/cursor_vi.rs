//! Vi-cursor extraction tests: entry anchoring, scroll clamping,
//! content-scroll and resize tracking, and the vi-mode gate.

use super::*;
use crate::schema::{GridLine, ViewportLine};

/// Asserts that `vi_cursor` is `None` outside vi mode.
///
/// Case: the terminal runs a normal shell session and no vi-mode
/// overlay exists for the renderer to draw.
#[test]
fn outside_vi_mode_there_is_no_vi_cursor() {
    let vt = AlacrittyVtBackend::new(GRID_COLS, GRID_ROWS);
    assert_eq!(vt.vi_cursor(), None);
}

/// Asserts that entering vi mode anchors the vi cursor at the write
/// cursor.
///
/// Case: the user presses the vi-mode shortcut at an idle prompt and
/// starts navigating from where the caret was.
#[test]
fn entering_vi_mode_anchors_at_the_write_cursor() {
    let mut vt = vt_after(b"abc");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(0, 3)));
}

/// Asserts that entering vi mode while the write cursor is scrolled
/// out of view anchors the vi cursor at the viewport top-left.
///
/// Case: the user scrolls back through history and then enters vi
/// mode; navigation starts inside the window being read, not at the
/// off-screen prompt.
#[test]
fn entering_vi_mode_while_scrolled_back_anchors_at_the_viewport_top_left() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(5));
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(-5, 0)));
}

/// Asserts that leaving vi mode removes the vi cursor and that the
/// next entry re-anchors at the current write cursor.
///
/// Case: the user leaves vi mode, the application moves its caret,
/// and a later vi-mode session starts from the new caret position
/// rather than a stale one.
#[test]
fn leaving_vi_mode_removes_the_vi_cursor() {
    let mut vt = vt_after(b"abc");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert!(vt.vi_cursor().is_some());
    vt.switch_vi_mode(ViModeSwitch::Exit).unwrap();
    assert_eq!(vt.vi_cursor(), None);
    vt.interpret(b"\x1b[10;20H");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(9, 19)));
}

/// Asserts that the vi cursor stays anchored while the write cursor
/// moves, and that a redundant enter request does not re-anchor it.
///
/// Case: the user inspects the screen in vi mode while the shell
/// keeps printing; the reading position must not jump to the output,
/// even if the input glue re-sends the enter request.
#[test]
fn the_vi_cursor_stays_while_the_write_cursor_moves() {
    let mut vt = vt_after(b"abc");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    vt.interpret(b"\x1b[10;20H");
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(0, 3)));
    assert_eq!(vt.cursor().point, point(9, 19));
    assert_eq!(vt.switch_vi_mode(ViModeSwitch::Enter).unwrap(), None);
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(0, 3)));
}

/// Asserts that scrolling clamps the vi cursor into the viewport,
/// and that a negative grid line can still be visible.
///
/// Case: the user scrolls to the top of scrollback while in vi mode;
/// the vi cursor follows into history and stays on a visible row, so
/// a consumer must project the line instead of reading its sign.
#[test]
fn scrolling_clamps_the_vi_cursor_into_the_viewport() {
    let mut vt = vt_with_history(30);
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(23, 0)));
    vt.scroll(Scroll::Top);
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(-7, 0)));
    assert_eq!(
        GridLine(-7).to_viewport(vt.display_offset(), GRID_ROWS),
        Some(ViewportLine(23))
    );
    vt.scroll(Scroll::Bottom);
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(0, 0)));
}

/// Asserts that new output moves the vi cursor with its text while
/// the write cursor keeps its screen position.
///
/// Case: the user holds the vi cursor on a log line while the shell
/// emits another line; the vi cursor follows the text upward instead
/// of pointing at whatever scrolled into its cell.
#[test]
fn output_scrolls_the_vi_cursor_with_its_text() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(23, 0)));
    vt.interpret(b"\r\n");
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(22, 0)));
    assert_eq!(vt.cursor().point, point(23, 0));
}

/// Asserts that a resize carries the vi cursor with the content and
/// clamps it to the new dimensions.
///
/// Case: the user shrinks the window while inspecting the screen in
/// vi mode, and the vi cursor lands on the surviving cell nearest to
/// where it was.
#[test]
fn resizing_carries_the_vi_cursor_with_the_content() {
    let mut vt = vt_after(b"\x1b[24;80H");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(23, 79)));
    vt.resize(40, 12);
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(11, 39)));
}

/// Asserts that a reverse index at the top row moves the vi cursor
/// down with its text.
///
/// Case: an application scrolls its content downward while the user
/// holds the vi cursor on a line; the vi cursor follows that line to
/// its new row.
#[test]
fn reverse_index_scrolls_the_vi_cursor_down_with_its_text() {
    let mut vt = vt_after(b"abc");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(0, 3)));
    vt.interpret(b"\x1b[H\x1bM");
    assert_eq!(vt.vi_cursor().map(|v| v.point), Some(point(1, 3)));
}
