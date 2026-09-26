//! Tests for the vi-mode operations: they arm the coalescer only on a real
//! change, and vi mode routes the mouse to the terminal's own selection and
//! scrollback.

use super::*;
use crate::input::{PointerButton, PointerInput, PointerKind};
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

fn pointer(
    kind: PointerKind,
    button: Option<PointerButton>,
    col: u32,
    side: CellSide,
) -> PointerInput {
    PointerInput {
        kind,
        button,
        cell: CellCoord { col, row: 1 },
        side,
        click_count: 1,
        mods: ProtocolModifiers::default(),
    }
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
    term.send_pointer(pointer(
        PointerKind::Press,
        Some(PointerButton::Left),
        1,
        CellSide::Left,
    ))
    .expect("send_pointer");
    term.send_pointer(pointer(PointerKind::Motion, None, 5, CellSide::Left))
        .expect("send_pointer");
    let copied = term
        .send_pointer(pointer(
            PointerKind::Release,
            Some(PointerButton::Left),
            5,
            CellSide::Left,
        ))
        .expect("send_pointer");
    term.settle_writes();
    assert_eq!(copied.as_deref(), Some("hello"));
    assert_eq!(sink.contents(), b"");
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
    term.send_pointer(pointer(
        PointerKind::Press,
        Some(PointerButton::Left),
        1,
        CellSide::Left,
    ))
    .expect("send_pointer");
    term.switch_vi_mode(ViModeSwitch::Enter);
    term.send_pointer(pointer(
        PointerKind::Release,
        Some(PointerButton::Left),
        1,
        CellSide::Left,
    ))
    .expect("send_pointer");
    term.settle_writes();
    let written = sink.contents();
    assert_eq!(written.iter().filter(|&&byte| byte == 0x1b).count(), 2);
    assert!(written.ends_with(b"m"));
}
