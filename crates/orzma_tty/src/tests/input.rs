//! Tests for the key, mouse, and paste writers: VT modes decide the
//! encoding, and a paste snaps a scrolled-back viewport.

use super::*;

/// Asserts the scroll-on-input integration: user input while
/// scrolled back snaps the viewport to the live tail AND schedules
/// the repaint of that snap.
///
/// Case: the user scrolls into history and then pastes at the prompt.
#[test]
fn paste_while_scrolled_back_snaps_and_arms() {
    let (mut term, _sink) = detached_term();
    term.vt.display_offset = DisplayOffset(3);
    term.send_paste("x").expect("send_paste");
    assert!(
        term.vt.scrolls.iter().any(|s| matches!(s, Scroll::Bottom)),
        "input must snap to the live tail"
    );
    assert_eq!(term.vt.display_offset, DisplayOffset(0));
    assert!(
        term.coalescer.is_armed(),
        "the snap must schedule a repaint"
    );
}

/// Asserts that key encoding consults the VT-reported DECCKM state.
///
/// Case: the user presses an arrow key in a full-screen app that
/// enabled application cursor keys, then again at a plain prompt.
#[test]
fn send_key_honours_the_vt_reported_cursor_mode() {
    let (mut term, sink) = detached_term();
    term.vt.modes.app_cursor = true;
    term.send_key(&TerminalKey::ArrowUp, &TerminalModifiers::default())
        .expect("send_key");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1bOA");

    let (mut term, sink) = detached_term();
    term.send_key(&TerminalKey::ArrowUp, &TerminalModifiers::default())
        .expect("send_key");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[A");
}

/// Asserts that `send_mouse` writes the encoded report while a mouse
/// tracking level is in force, and writes nothing while none is.
///
/// Case: a client of the multiplexer's command channel forwards a
/// button report for a pane in which nvim has just exited, dropping
/// DECRST 1002 and 1006 before the report arrives.
#[test]
fn send_mouse_only_writes_while_a_tracking_level_is_in_force() {
    let report = MouseReport {
        button: MouseButton::WheelUp,
        kind: MouseReportKind::Press,
        cell: CellCoord { col: 1, row: 1 },
        mods: ProtocolModifiers::default(),
    };

    let (mut term, sink) = tracking_term();
    assert_eq!(sink.contents(), b"", "construction must write nothing");
    term.send_mouse(report).expect("send_mouse");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<64;1;1M");

    let (mut term, sink) = detached_term();
    assert_eq!(sink.contents(), b"", "construction must write nothing");
    term.send_mouse(report).expect("send_mouse");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}

/// Asserts that paste encoding consults the VT-reported bracketed
/// paste mode.
///
/// Case: the user pastes into an app that enabled DECSET 2004, such
/// as vim, fzf, or a modern shell.
#[test]
fn send_paste_honours_the_vt_reported_bracketed_mode() {
    let (mut term, sink) = detached_term();
    term.vt.modes.bracketed_paste = true;
    term.send_paste("hi").expect("send_paste");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[200~hi\x1b[201~");
}

/// Asserts that a detached terminal's PTY writes land on the
/// injected sink, byte-identical.
///
/// Case: a caller builds a terminal with an injected writer instead
/// of a spawned shell, then reads back the bytes the terminal
/// produced.
#[test]
fn detached_routes_writes_to_the_injected_sink() {
    let (mut term, sink) = detached_term();
    term.send_paste("hi").expect("send_paste");
    term.settle_writes();
    assert_eq!(sink.contents(), b"hi");
}

/// Asserts that `send_paste("")` writes nothing at all, rather than an
/// empty bracketed-paste frame.
///
/// Case: the user pastes with an empty clipboard.
#[test]
fn empty_paste_writes_nothing_to_the_pty() {
    let (mut term, sink) = detached_term();
    term.send_paste("").expect("send_paste");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}
