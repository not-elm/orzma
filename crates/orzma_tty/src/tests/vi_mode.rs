//! Tests for the vi-mode operations: they arm the coalescer only on a real
//! change, and vi mode routes the mouse to the terminal's own selection and
//! scrollback.

use super::*;
use orzma_vt::prelude::{DisplayOffset, OrzmaVt, SelectionKind, ViModeSwitch, ViMotion};

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
    term.vi_motion(ViMotion::Left);
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

/// Asserts that the wheel in vi mode scrolls the scrollback and reports
/// nothing, even while the application tracks the mouse.
///
/// Case: a program that tracks the mouse has printed more than a screenful,
/// and the user enters vi mode and turns the wheel up.
#[test]
fn the_wheel_in_vi_mode_scrolls_while_the_app_tracks_the_mouse() {
    let (mut term, sink) = real_term(3);
    term.feed_bytes(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[?1000h\x1b[?1006h")
        .expect("the VT interprets the bytes");
    term.switch_vi_mode(ViModeSwitch::Enter);
    let wheel = WheelInput {
        up: 1,
        right: 0,
        mods: WheelModifiers::default(),
        cell: Some(CellCoord { col: 1, row: 1 }),
        report_mods: ProtocolModifiers::default(),
    };
    term.send_wheel(wheel, &WheelConfig::default())
        .expect("send_wheel");
    term.settle_writes();
    assert_ne!(term.vt.display_offset(), DisplayOffset(0));
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
