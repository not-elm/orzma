//! Tests for the key and paste writers: VT modes decide the encoding, and
//! input snaps a scrolled-back viewport and dismisses the selection.

use super::*;
use crate::input::KeyText;
use crate::test_support::SelectionOp;

/// Asserts that a key sent to the PTY dismisses the selection and
/// schedules the repaint that removes its highlight.
///
/// Case: the user drags over some output to select it, then starts typing
/// the next command at the prompt.
#[test]
fn send_key_dismisses_the_selection_and_arms() {
    let (mut term, _sink) = detached_term();
    term.vt.selection_changes = true;
    term.send_key(
        &TerminalKey::Character(KeyText::new("a").expect("a non-empty key text")),
        &TerminalModifiers::default(),
    )
    .expect("send_key");
    assert_eq!(term.vt.selections, vec![SelectionOp::Clear]);
    assert!(
        term.coalescer.is_armed(),
        "the dismissal must schedule a repaint"
    );
}

/// Asserts that a non-empty paste dismisses the selection and schedules
/// the repaint that removes its highlight.
///
/// Case: the user selects a path in the output with the mouse, then pastes
/// a command from the clipboard at the prompt.
#[test]
fn send_paste_dismisses_the_selection_and_arms() {
    let (mut term, _sink) = detached_term();
    term.vt.selection_changes = true;
    term.send_paste("ls").expect("send_paste");
    assert_eq!(term.vt.selections, vec![SelectionOp::Clear]);
    assert!(
        term.coalescer.is_armed(),
        "the dismissal must schedule a repaint"
    );
}

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

/// Asserts that an empty paste leaves the selection in place.
///
/// Case: the user selects some output with the mouse, then presses the
/// paste shortcut while the clipboard is empty.
#[test]
fn empty_paste_keeps_the_selection() {
    let (mut term, _sink) = detached_term();
    term.vt.selection_changes = true;
    term.send_paste("").expect("send_paste");
    assert!(term.vt.selections.is_empty());
    assert!(!term.coalescer.is_armed());
}
