//! Tests for the vi-mode operations: the coalescer arms only on a real
//! change, the mouse drives the terminal's own selection and scrollback, and
//! scrolls reach a mouse-tracking alternate-screen application as wheel reports.

use super::*;
use orzma_vt::prelude::{DisplayOffset, OrzmaVt, SelectionKind, ViModeSwitch, ViMotion};

/// The output that puts a 10-column terminal on the alternate screen with
/// SGR button-event tracking, prints `hello` on row 3, and parks the write
/// cursor on its `e`.
const APP_SCROLL_SETUP: &[u8] = b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[3;1Hhello\x1b[3;2H";

/// A 10-column terminal `rows` tall over a real VT, plus the sink its PTY
/// writes land on.
fn real_term(rows: u16) -> (OrzmaTty<OrzmaVt>, CaptureSink) {
    let sink = CaptureSink::default();
    let term = OrzmaTty::detached(
        OrzmaVt::new(grid(10, rows), 100),
        grid(10, rows),
        Box::new(sink.clone()),
    )
    .expect("OrzmaTty::detached");
    (term, sink)
}

/// A terminal `rows` tall that has interpreted [`APP_SCROLL_SETUP`] and
/// entered vi mode, plus the sink its PTY writes land on. With at least
/// three rows the vi cursor sits on line 2, column 1.
fn app_scroll_term(rows: u16) -> (OrzmaTty<OrzmaVt>, CaptureSink) {
    let (mut term, sink) = real_term(rows);
    term.feed_bytes(APP_SCROLL_SETUP)
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    (term, sink)
}

/// The terminal [`app_scroll_term`] builds, with a PTY writer that fails
/// every write.
fn failing_app_scroll_term(rows: u16) -> OrzmaTty<OrzmaVt> {
    let mut term = OrzmaTty::detached(
        OrzmaVt::new(grid(10, rows), 100),
        grid(10, rows),
        Box::new(FailingSink),
    )
    .expect("OrzmaTty::detached");
    term.feed_bytes(APP_SCROLL_SETUP)
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term
}

/// Asserts that entering vi mode arms the coalescer, and that a repeated
/// enter does not.
///
/// Case: the user presses the vi-mode shortcut twice on an idle terminal.
#[test]
fn entering_vi_mode_arms_the_coalescer_once() {
    let (mut term, _sink) = real_term(3);
    let _ = term.pump();
    term.coalescer.disarm();
    term.switch_vi_mode(ViModeSwitch::Enter);
    assert!(term.coalescer.is_armed());
    term.coalescer.disarm();
    term.switch_vi_mode(ViModeSwitch::Enter);
    assert!(!term.coalescer.is_armed());
}

/// Asserts that a motion and a toggle in vi mode arm the coalescer.
///
/// Case: the user presses `h` and then `v` in vi mode on an idle terminal.
#[test]
fn a_motion_and_a_toggle_arm_the_coalescer() {
    let (mut term, _sink) = real_term(3);
    term.feed_bytes(b"abc")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.coalescer.disarm();
    term.vi_motion(ViMotion::Left).expect("vi_motion");
    assert!(term.coalescer.is_armed());
    term.coalescer.disarm();
    term.toggle_vi_selection(SelectionKind::Simple);
    assert!(term.coalescer.is_armed());
}

/// Asserts that a drag in vi mode selects text and reports nothing, even
/// while the application tracks the mouse.
///
/// Case: nvim has turned on button tracking, and the user enters vi mode
/// and drags across a word to copy it.
#[test]
fn a_drag_in_vi_mode_selects_while_the_app_tracks_the_mouse() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"hello\x1b[?1002h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.send_pointer(motion(5, 1)).expect("send_pointer");
    let copied = term
        .send_pointer(release(PointerButton::Left, 5, 1))
        .expect("send_pointer");
    term.settle_writes();
    assert_eq!(copied.as_deref(), Some("hello"));
    assert_eq!(sink.contents(), b"");
}

/// Asserts that an unmoved double click in vi mode copies nothing and
/// leaves no selection that covers a cell.
///
/// Case: the user enters vi mode at a shell prompt showing a word and
/// double-clicks that word without moving the pointer.
#[test]
fn an_unmoved_double_click_in_vi_mode_copies_nothing() {
    let (mut term, _sink) = real_term(3);
    term.feed_bytes(b"hello")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    for click_count in [1, 2] {
        term.send_pointer(PointerInput {
            click_count,
            ..press(PointerButton::Left, 3, 1)
        })
        .expect("send_pointer");
        let copied = term
            .send_pointer(release(PointerButton::Left, 3, 1))
            .expect("send_pointer");
        assert_eq!(copied, None, "click {click_count}");
    }
    assert_eq!(term.vt.selection_text(), None);
}

/// Asserts that the wheel in vi mode scrolls the scrollback by the
/// notches rather than the reports, and reports nothing, even while the
/// application tracks the mouse.
///
/// Case: a program that tracks the mouse has printed more than a screenful,
/// and the user enters vi mode and turns the wheel up one notch while the
/// frame carries five reports.
#[test]
fn the_wheel_in_vi_mode_scrolls_by_the_notches_while_the_app_tracks_the_mouse() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[?1000h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    let input = WheelInput {
        steps: WheelSteps {
            up: 1,
            report_up: 5,
            report_right: 0,
        },
        ..wheel(0, 0)
    };
    let cfg = WheelConfig {
        lines_per_notch: 1,
        ..WheelConfig::default()
    };
    term.send_wheel(input, &cfg).expect("send_wheel");
    term.settle_writes();
    assert_eq!(term.vt.display_offset(), DisplayOffset(1));
    assert_eq!(sink.contents(), b"");
}

/// Asserts that a press the application received before vi mode began
/// still has its release reported.
///
/// Case: the user holds the button in a program that tracks the mouse,
/// enters vi mode from the keyboard, and then lets go.
#[test]
fn a_release_after_entering_vi_mode_still_reaches_the_app() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"\x1b[?1000h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.send_pointer(release(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.settle_writes();
    let written = sink.contents();
    assert_eq!(written.iter().filter(|&&byte| byte == 0x1b).count(), 2);
    assert!(written.ends_with(b"m"));
}

/// Asserts that a vi page motion that cannot move the viewport still moves
/// the vi cursor while a mouse drag is held.
///
/// Case: in vi mode at the live tail the user holds a drag across a word
/// and presses `Ctrl+D`.
#[test]
fn a_page_motion_during_a_held_drag_moves_the_vi_cursor() {
    let (mut term, _sink) = real_term(4);
    term.feed_bytes(b"ab\r\ncd\r\nef\r\ngh")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.send_pointer(motion(2, 1)).expect("send_pointer");
    term.scroll(Scroll::HalfPageDown);
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(2))
    );
}

/// Asserts that a half-page scroll in vi mode over an alternate-screen
/// application that tracks the mouse sends wheel-up reports at the vi
/// cursor in one write, instead of moving the vi cursor or the viewport.
///
/// Case: the user enters vi mode over Claude Code's fullscreen chat and
/// presses `Ctrl+U` to read back older messages.
#[test]
fn a_half_page_up_over_a_mouse_tracking_app_sends_wheel_ups_at_the_vi_cursor() {
    let (mut term, sink) = app_scroll_term(12);
    term.vi_scroll(Scroll::HalfPageUp).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;2;3M\x1b[<64;2;3M".to_vec());
    assert_eq!(sink.writes(), 1);
    assert!(term.vt.is_vi_mode());
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point),
        Some(GridPoint {
            line: GridLine(2),
            column: GridColumn(1)
        })
    );
    assert_eq!(term.vt.display_offset(), DisplayOffset(0));
}

/// Asserts that each forwarded vi scroll sends its count of wheel reports
/// in its direction, at least one for a page on a one-row pane and no more
/// than a page's worth for a long line scroll.
///
/// Case: over Claude Code's fullscreen chat the user presses each of the
/// vi scroll keys, in panes of several heights.
#[test]
fn each_vi_scroll_kind_sends_its_wheel_report_count() {
    for (rows, scroll, button, count) in [
        (12, Scroll::HalfPageDown, 65, 2),
        (12, Scroll::PageUp, 64, 4),
        (12, Scroll::PageDown, 65, 4),
        (12, Scroll::Delta(1), 64, 1),
        (12, Scroll::Delta(-1), 65, 1),
        (24, Scroll::HalfPageUp, 64, 4),
        (1, Scroll::HalfPageUp, 64, 1),
        (1, Scroll::PageDown, 65, 1),
        (12, Scroll::Delta(100), 64, 4),
        (12, Scroll::Delta(i32::MIN), 65, 4),
    ] {
        let (mut term, sink) = app_scroll_term(rows);
        term.vi_scroll(scroll).expect("vi_scroll");
        term.settle_writes();
        let row = rows.min(3);
        let report = format!("\x1b[<{button};2;{row}M");
        assert_eq!(
            sink.contents(),
            report.repeat(count).into_bytes(),
            "{rows} rows, {scroll:?}"
        );
    }
}

/// Asserts that the history jumps and a zero line scroll are not forwarded
/// and keep the selection.
///
/// Case: over Claude Code's fullscreen chat the user has selected a word in
/// vi mode and presses `g` or `G`.
#[test]
fn history_jumps_and_a_zero_delta_are_not_forwarded() {
    for scroll in [Scroll::Top, Scroll::Bottom, Scroll::Delta(0)] {
        let (mut term, sink) = app_scroll_term(12);
        term.toggle_vi_selection(SelectionKind::Simple);
        term.vi_scroll(scroll).expect("vi_scroll");
        term.settle_writes();
        assert_eq!(sink.contents(), b"", "{scroll:?}");
        assert!(term.vt.selection_text().is_some(), "{scroll:?}");
    }
}

/// Asserts that a forwarded vi scroll clears the selection and leaves vi
/// mode on.
///
/// Case: the user has selected a word in vi mode over Claude Code's
/// fullscreen chat and presses `Ctrl+U` before copying it.
#[test]
fn a_forwarded_vi_scroll_clears_the_selection_and_stays_in_vi_mode() {
    let (mut term, _sink) = app_scroll_term(12);
    term.toggle_vi_selection(SelectionKind::Simple);
    assert_eq!(term.vt.selection_text().as_deref(), Some("e"));
    term.vi_scroll(Scroll::HalfPageUp).expect("vi_scroll");
    assert_eq!(term.vt.selection_text(), None);
    assert!(term.vt.is_vi_mode());
}

/// Asserts that a forwarded vi scroll is encoded in the mouse encoding the
/// application chose.
///
/// Case: a full-screen program that tracks the mouse without asking for
/// SGR reports scrolls under `Ctrl+Y` in vi mode.
#[test]
fn a_forwarded_vi_scroll_uses_the_apps_mouse_encoding() {
    let (mut term, sink) = real_term(12);
    term.feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[3;2H")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[M`\"#".to_vec());
}

/// Asserts that a forwarded vi scroll reports the body column of the wide
/// glyph under the vi cursor.
///
/// Case: the vi cursor sits on a Japanese character in Claude Code's
/// fullscreen chat when the user presses `Ctrl+Y`.
#[test]
fn a_forwarded_vi_scroll_reports_the_body_column_of_a_wide_glyph() {
    let (mut term, sink) = real_term(12);
    term.feed_bytes("\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[3;1Hあい\x1b[3;4H".as_bytes())
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;3;3M".to_vec());
}

/// Asserts that after a forward the vi cursor follows the text when the
/// application redraws with a region scroll.
///
/// Case: in vi mode the user presses `Ctrl+Y` over a program that answers
/// the wheel with a scroll-down of its screen, as nvim does.
#[test]
fn an_app_region_scroll_after_a_forward_moves_the_vi_cursor_with_the_text() {
    let (mut term, sink) = app_scroll_term(12);
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;2;3M".to_vec());
    term.feed_bytes(b"\x1b[T")
        .expect("the VT interprets the bytes");
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(3))
    );
}

/// Asserts that vi scrolls stay on the terminal's own screen while a mouse
/// selection press is held, and reach the application again after its
/// release.
///
/// Case: the user drags a selection over Claude Code's fullscreen chat in
/// vi mode, presses `Ctrl+Y` mid-drag, and presses it again after letting
/// go.
#[test]
fn vi_scrolls_stay_local_while_a_selection_press_is_held() {
    let (mut term, sink) = app_scroll_term(12);
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.send_pointer(motion(4, 1)).expect("send_pointer");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    term.send_pointer(release(PointerButton::Left, 4, 1))
        .expect("send_pointer");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    let written = sink.contents();
    assert!(written.starts_with(b"\x1b[<64;") && written.ends_with(b"M"));
    assert_eq!(written.iter().filter(|&&byte| byte == 0x1b).count(), 1);
}

/// Asserts that vi scrolls over an alternate-screen program that does not
/// track the mouse move the vi cursor and reach nothing.
///
/// Case: the user enters vi mode over less and presses `Ctrl+U`.
#[test]
fn vi_scrolls_over_an_app_that_does_not_track_the_mouse_are_not_forwarded() {
    let (mut term, sink) = real_term(12);
    term.feed_bytes(b"\x1b[?1049h\x1b[3;2H")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.vi_scroll(Scroll::HalfPageUp).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert!(
        term.vt
            .vi_cursor()
            .is_some_and(|cursor| cursor.point.line.0 < 2)
    );
}

/// Asserts that vi scrolls on the primary screen scroll the scrollback and
/// send nothing, even while the application tracks the mouse.
///
/// Case: a shell tool that tracks the mouse has printed more than a
/// screenful, and the user presses `Ctrl+Y` in vi mode.
#[test]
fn vi_scrolls_on_the_primary_screen_scroll_the_scrollback_while_the_app_tracks_the_mouse() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[?1002h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(term.vt.display_offset(), DisplayOffset(1));
    assert_eq!(sink.contents(), b"");
}

/// Asserts that once the application leaves the alternate screen, vi
/// scrolls scroll the scrollback instead of reaching the application.
///
/// Case: the user quits Claude Code while vi mode stays on, then presses
/// `Ctrl+Y` to read the shell history.
#[test]
fn a_vi_scroll_after_the_app_leaves_the_alternate_screen_is_not_forwarded() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[?1049h\x1b[?1002h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.feed_bytes(b"\x1b[?1049l")
        .expect("the VT interprets the bytes");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert_eq!(term.vt.display_offset(), DisplayOffset(1));
}

/// Asserts that once the application stops tracking the mouse, vi scrolls
/// no longer reach it.
///
/// Case: a full-screen program turns mouse tracking off while the user is
/// in vi mode, and the user presses `Ctrl+Y`.
#[test]
fn a_vi_scroll_after_the_app_stops_tracking_the_mouse_is_not_forwarded() {
    let (mut term, sink) = app_scroll_term(12);
    term.feed_bytes(b"\x1b[?1002l")
        .expect("the VT interprets the bytes");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}

/// Asserts that vi scrolls reach an application that enters the alternate
/// screen and tracks the mouse after vi mode began.
///
/// Case: the user enters vi mode at the shell prompt, Claude Code starts in
/// the pane, and the user presses `Ctrl+Y`.
#[test]
fn a_vi_scroll_after_the_app_enters_the_alternate_screen_is_forwarded() {
    let (mut term, sink) = real_term(12);
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    let written = sink.contents();
    assert!(written.starts_with(b"\x1b[<64;") && written.ends_with(b"M"));
}

/// Asserts that a forwarded vi scroll the PTY refuses is reported as an
/// error and sets no key-forward deadline, so the next one is not held back.
///
/// Case: Claude Code's PTY fails a write, and the user keeps pressing
/// `Ctrl+Y` in vi mode.
#[test]
fn a_refused_vi_scroll_forward_is_reported() {
    let mut term = failing_app_scroll_term(12);
    term.vi_scroll(Scroll::Delta(1))
        .expect("the reports are queued before the writer fails");
    term.settle_writes();
    term.key_forward_deadline = None;
    assert!(matches!(
        term.vi_scroll(Scroll::Delta(1)),
        Err(OrzmaTtyError::PtyWrite(_))
    ));
    assert_eq!(term.key_forward_deadline, None);
    assert!(matches!(
        term.vi_scroll(Scroll::Delta(1)),
        Err(OrzmaTtyError::PtyWriterClosed)
    ));
}

/// Asserts that an `Up` on the top row over an alternate-screen
/// application that tracks the mouse sends one wheel-up report and leaves
/// the vi cursor where it is.
///
/// Case: the user holds `k` in vi mode at the top of Claude Code's
/// fullscreen chat.
#[test]
fn cursor_up_on_the_top_row_over_a_tracking_app_sends_one_wheel_up() {
    let (mut term, sink) = real_term(12);
    term.feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[1;2H")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.vi_motion(ViMotion::Up).expect("vi_motion");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;2;1M".to_vec());
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(0))
    );
}

/// Asserts that a `Down` on the bottom row over an alternate-screen
/// application that tracks the mouse sends one wheel-down report and
/// leaves the vi cursor where it is.
///
/// Case: the user presses `j` in vi mode at the bottom of Claude Code's
/// fullscreen chat to read toward the newest message.
#[test]
fn cursor_down_on_the_bottom_row_over_a_tracking_app_sends_one_wheel_down() {
    let (mut term, sink) = real_term(12);
    term.feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[12;2H")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.vi_motion(ViMotion::Down).expect("vi_motion");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<65;2;12M".to_vec());
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(11))
    );
}

/// Asserts that an `Up` below the top row moves the vi cursor and sends
/// nothing.
///
/// Case: the user presses `k` in vi mode in the middle of Claude Code's
/// fullscreen chat.
#[test]
fn cursor_up_below_the_top_row_moves_the_vi_cursor_without_forwarding() {
    let (mut term, sink) = app_scroll_term(12);
    term.vi_motion(ViMotion::Up).expect("vi_motion");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(1))
    );
}

/// Asserts that an edge forward the PTY refuses is reported as an error
/// and sets no key-forward deadline.
///
/// Case: Claude Code's PTY fails a write, and the user keeps pressing `k`
/// at the top of its chat in vi mode.
#[test]
fn a_refused_edge_forward_is_reported() {
    let mut term = failing_app_scroll_term(12);
    term.vi_motion(ViMotion::High).expect("vi_motion");
    term.vi_motion(ViMotion::Up)
        .expect("the report is queued before the writer fails");
    term.settle_writes();
    term.key_forward_deadline = None;
    assert!(matches!(
        term.vi_motion(ViMotion::Up),
        Err(OrzmaTtyError::PtyWrite(_))
    ));
    assert_eq!(term.key_forward_deadline, None);
}

/// Asserts that an edge forward clears the selection and leaves vi mode
/// on.
///
/// Case: the user has selected a word on the top line of Claude Code's
/// fullscreen chat in vi mode and presses `k`.
#[test]
fn an_edge_forward_clears_the_selection() {
    let (mut term, _sink) = real_term(12);
    term.feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[1;1Hhello\x1b[1;2H")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.toggle_vi_selection(SelectionKind::Simple);
    assert_eq!(term.vt.selection_text().as_deref(), Some("e"));
    term.vi_motion(ViMotion::Up).expect("vi_motion");
    assert_eq!(term.vt.selection_text(), None);
    assert!(term.vt.is_vi_mode());
}

/// Asserts that an `Up` on the top row stays on the terminal's own screen
/// while a mouse selection press is held.
///
/// Case: the user presses the mouse on the top line of Claude Code's
/// fullscreen chat in vi mode and presses `k` before letting go.
#[test]
fn cursor_up_on_the_top_row_stays_local_while_a_selection_press_is_held() {
    let (mut term, sink) = app_scroll_term(12);
    term.send_pointer(press(PointerButton::Left, 2, 1))
        .expect("send_pointer");
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(0))
    );
    term.vi_motion(ViMotion::Up).expect("vi_motion");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}

/// Asserts that a forward holds the next one back for exactly
/// `KEY_FORWARD_INTERVAL`: one a microsecond sooner is dropped without
/// moving the deadline, and one at the deadline goes out.
///
/// Case: the user holds `Ctrl+Y` in vi mode over Claude Code's fullscreen
/// chat, and the key repeats on both sides of the interval.
#[test]
fn a_forward_holds_the_next_back_for_exactly_the_key_forward_interval() {
    let (mut term, sink) = app_scroll_term(12);
    let up = WheelDecision::Report {
        button: MouseButton::WheelUp,
        count: 1,
    };
    let start = Instant::now();
    let deadline = start + OrzmaTty::<OrzmaVt>::KEY_FORWARD_INTERVAL;
    term.forward_wheel_to_app(up.clone(), start)
        .expect("forward_wheel_to_app");
    assert_eq!(term.key_forward_deadline, Some(deadline));
    term.forward_wheel_to_app(up.clone(), deadline - Duration::from_micros(1))
        .expect("forward_wheel_to_app");
    assert_eq!(term.key_forward_deadline, Some(deadline));
    term.forward_wheel_to_app(up, deadline)
        .expect("forward_wheel_to_app");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;2;3M".repeat(2));
}

/// Asserts that a forwarded vi scroll and a forwarded edge motion each hold
/// the next forward back until `KEY_FORWARD_INTERVAL` after they were sent.
///
/// Case: over Claude Code's fullscreen chat in vi mode the user presses
/// `Ctrl+Y` once, or `k` once on the top line.
#[test]
fn vi_scroll_and_edge_forwards_start_the_key_forward_interval() {
    let interval = OrzmaTty::<OrzmaVt>::KEY_FORWARD_INTERVAL;
    let (mut scrolled, _sink) = app_scroll_term(12);
    let before = Instant::now();
    scrolled.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    let after = Instant::now();
    let deadline = scrolled
        .key_forward_deadline
        .expect("a forwarded vi scroll sets a deadline");
    assert!(before + interval <= deadline && deadline <= after + interval);
    let (mut moved, _sink) = real_term(12);
    moved
        .feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[1;2H")
        .expect("the VT interprets the bytes");
    moved.switch_vi_mode(ViModeSwitch::Enter);
    let before = Instant::now();
    moved.vi_motion(ViMotion::Up).expect("vi_motion");
    let after = Instant::now();
    let deadline = moved
        .key_forward_deadline
        .expect("a forwarded edge motion sets a deadline");
    assert!(before + interval <= deadline && deadline <= after + interval);
}

/// Asserts that a vi scroll before the key-forward deadline is dropped
/// rather than queued, and keeps the selection and the deadline.
///
/// Case: the user holds `Ctrl+Y` in vi mode over Claude Code's fullscreen
/// chat, and the key repeats faster than the interval.
#[test]
fn a_vi_scroll_before_the_key_forward_deadline_is_dropped() {
    let (mut term, sink) = app_scroll_term(12);
    let held = Instant::now() + Duration::from_secs(60);
    term.key_forward_deadline = Some(held);
    term.toggle_vi_selection(SelectionKind::Simple);
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert_eq!(term.vt.selection_text().as_deref(), Some("e"));
    assert_eq!(term.key_forward_deadline, Some(held));
}

/// Asserts that an edge motion before the key-forward deadline is dropped,
/// leaving the vi cursor on the top row.
///
/// Case: the user holds `k` in vi mode at the top of Claude Code's
/// fullscreen chat right after pressing `Ctrl+Y`.
#[test]
fn an_edge_motion_before_the_key_forward_deadline_is_dropped() {
    let (mut term, sink) = real_term(12);
    term.feed_bytes(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[1;2H")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.key_forward_deadline = Some(Instant::now() + Duration::from_secs(60));
    term.vi_motion(ViMotion::Up).expect("vi_motion");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert_eq!(
        term.vt.vi_cursor().map(|cursor| cursor.point.line),
        Some(GridLine(0))
    );
}

/// Asserts that the wheel is neither held back by the key-forward deadline
/// nor sets one.
///
/// Case: in vi mode over Claude Code's fullscreen chat the user spins the
/// wheel, presses `Ctrl+Y`, and spins the wheel again at once.
#[test]
fn the_wheel_ignores_the_key_forward_deadline() {
    let (mut term, sink) = app_scroll_term(12);
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(term.key_forward_deadline, None);
    let held = Instant::now() + Duration::from_secs(60);
    term.key_forward_deadline = Some(held);
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;6;4M".repeat(2));
    assert_eq!(term.key_forward_deadline, Some(held));
}

/// Asserts that a vi scroll that stays on the terminal's own screen is not
/// held back by the key-forward deadline.
///
/// Case: the user is holding `Ctrl+Y` over Claude Code's fullscreen chat in
/// vi mode when Claude Code exits, and the held key goes on to scroll the
/// shell history.
#[test]
fn a_local_vi_scroll_ignores_the_key_forward_deadline() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[?1049h\x1b[?1002h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.key_forward_deadline = Some(Instant::now() + Duration::from_secs(60));
    term.feed_bytes(b"\x1b[?1049l")
        .expect("the VT interprets the bytes");
    term.vi_scroll(Scroll::Delta(1)).expect("vi_scroll");
    term.settle_writes();
    assert_eq!(term.vt.display_offset(), DisplayOffset(1));
    assert_eq!(sink.contents(), b"");
}

/// Asserts that the wheel in vi mode over an alternate-screen application
/// that tracks the mouse sends its reports at the pointer, on both axes.
///
/// Case: the user scrolls Claude Code's fullscreen chat with the mouse
/// wheel while in vi mode, then tilts the wheel sideways.
#[test]
fn the_wheel_in_vi_mode_over_a_tracking_app_sends_reports_at_the_pointer() {
    let (mut term, sink) = app_scroll_term(12);
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    term.send_wheel(wheel(0, 1), &WheelConfig::default())
        .expect("send_wheel");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;6;4M\x1b[<67;6;4M".to_vec());
}

/// Asserts that the wheel in vi mode on the alternate screen never becomes
/// cursor keys, whatever the tracking, alternate-scroll and Shift state,
/// and reaches the application as reports only while it tracks the mouse
/// and Shift is not held.
///
/// Case: in vi mode the user spins the wheel, with and without Shift, over
/// Claude Code's fullscreen chat and over less.
#[test]
fn the_wheel_in_vi_mode_on_the_alternate_screen_never_sends_cursor_keys() {
    for tracking in [&b""[..], b"\x1b[?1002h\x1b[?1006h"] {
        for alternate_scroll in [&b"\x1b[?1007h"[..], b"\x1b[?1007l"] {
            for shift in [false, true] {
                for (up, button) in [(1, 64), (-1, 65)] {
                    let (mut term, sink) = real_term(12);
                    term.feed_bytes(b"\x1b[?1049h\x1b[?1h")
                        .expect("the VT interprets the bytes");
                    term.feed_bytes(tracking)
                        .expect("the VT interprets the bytes");
                    term.feed_bytes(alternate_scroll)
                        .expect("the VT interprets the bytes");
                    term.switch_vi_mode(ViModeSwitch::Enter);
                    let input = WheelInput {
                        mods: WheelModifiers { shift, fine: false },
                        ..wheel(up, 0)
                    };
                    term.send_wheel(input, &WheelConfig::default())
                        .expect("send_wheel");
                    term.settle_writes();
                    let expected = if !tracking.is_empty() && !shift {
                        format!("\x1b[<{button};6;4M").into_bytes()
                    } else {
                        Vec::new()
                    };
                    assert_eq!(
                        sink.contents(),
                        expected,
                        "tracking {}, {:?}, shift {shift}, up {up}",
                        !tracking.is_empty(),
                        String::from_utf8_lossy(alternate_scroll)
                    );
                }
            }
        }
    }
}

/// Asserts that the wheel in vi mode keeps the report cap, the fine
/// modifier and the report modifier bits of the wheel outside vi mode, at
/// every mouse tracking level.
///
/// Case: in vi mode the user spins the wheel hard, then holds Option and
/// spins it once, over programs that track clicks, drags, or every motion.
#[test]
fn the_wheel_in_vi_mode_keeps_the_outside_report_rules_at_every_tracking_level() {
    for tracking in [&b"\x1b[?1000h"[..], b"\x1b[?1002h", b"\x1b[?1003h"] {
        let (mut term, sink) = real_term(12);
        term.feed_bytes(b"\x1b[?1049h\x1b[?1006h")
            .expect("the VT interprets the bytes");
        term.feed_bytes(tracking)
            .expect("the VT interprets the bytes");
        term.switch_vi_mode(ViModeSwitch::Enter);
        let capped = WheelConfig {
            max_protocol_events_per_frame: 2,
            ..WheelConfig::default()
        };
        term.send_wheel(wheel(5, 0), &capped).expect("send_wheel");
        let fine_with_alt = WheelInput {
            mods: WheelModifiers {
                shift: false,
                fine: true,
            },
            report_mods: ProtocolModifiers {
                alt: true,
                ..ProtocolModifiers::default()
            },
            ..wheel(1, 0)
        };
        term.send_wheel(fine_with_alt, &WheelConfig::default())
            .expect("send_wheel");
        term.settle_writes();
        assert_eq!(
            sink.contents(),
            b"\x1b[<64;6;4M\x1b[<64;6;4M\x1b[<72;6;4M".to_vec(),
            "{}",
            String::from_utf8_lossy(tracking)
        );
    }
}

/// Asserts that a wheel frame that sends reports in vi mode clears the
/// selection and leaves vi mode on.
///
/// Case: the user has selected a word in vi mode over Claude Code's
/// fullscreen chat and spins the wheel.
#[test]
fn a_forwarded_wheel_clears_the_selection() {
    let (mut term, _sink) = app_scroll_term(12);
    term.toggle_vi_selection(SelectionKind::Simple);
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(term.vt.selection_text(), None);
    assert!(term.vt.is_vi_mode());
}

/// Asserts that a wheel frame with no cell under the pointer sends nothing
/// and keeps the selection.
///
/// Case: the user spins the wheel over the edge of the pane, where no cell
/// is resolved, while a vi selection is open over Claude Code's chat.
#[test]
fn a_wheel_without_a_cell_keeps_the_selection() {
    let (mut term, sink) = app_scroll_term(12);
    term.toggle_vi_selection(SelectionKind::Simple);
    let input = WheelInput {
        cell: None,
        ..wheel(1, 0)
    };
    term.send_wheel(input, &WheelConfig::default())
        .expect("send_wheel");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert!(term.vt.selection_text().is_some());
}

/// Asserts that the wheel does nothing on the alternate screen while a
/// mouse selection press is held, before and after the pointer moves.
///
/// Case: the user drags a selection over Claude Code's fullscreen chat in
/// vi mode and spins the wheel mid-drag.
#[test]
fn the_wheel_during_a_held_selection_press_does_nothing_on_the_alternate_screen() {
    let (mut term, sink) = app_scroll_term(12);
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    term.send_pointer(motion(4, 1)).expect("send_pointer");
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}
