//! Tests for `scroll` and the selection operations: only a real
//! viewport or selection change arms the coalescer.

use super::*;

/// Asserts that a scroll the VT reports as a real move arms the
/// coalescer.
///
/// Case: the user scrolls into history on an idle terminal, where
/// the viewport change is the only thing that happens.
#[test]
fn scroll_arms_the_coalescer_when_the_viewport_moves() {
    let (mut term, _sink) = detached_term();
    term.vt.scroll_moves = true;
    term.scroll(Scroll::Delta(3));
    assert!(term.coalescer.is_armed());
}

/// Asserts that a scroll the VT reports as a no-op arms nothing.
///
/// Case: the user keeps turning the wheel after the viewport
/// reached the end of the scrollback.
#[test]
fn a_no_op_scroll_does_not_arm_the_coalescer() {
    let (mut term, _sink) = detached_term();
    term.scroll(Scroll::Delta(5));
    assert!(!term.coalescer.is_armed());
}

/// Asserts that a no-op scroll leaves an already-open emit window's
/// deadline untouched.
///
/// Case: the user keeps spinning the wheel at the clamp while an
/// earlier repaint is still pending.
#[test]
fn a_no_op_scroll_does_not_extend_the_deadline() {
    let (mut term, _sink) = detached_term();
    term.vt.scroll_moves = true;
    term.scroll(Scroll::Delta(3));
    let deadline = term.coalescer.next_deadline();
    assert!(deadline.is_some(), "precondition: a real scroll arms");
    term.vt.scroll_moves = false;
    term.scroll(Scroll::Delta(0));
    assert_eq!(term.coalescer.next_deadline(), deadline);
}

/// Asserts that scrolling writes nothing through the PTY writer,
/// neither a CSI S/T pair nor arrow keys.
///
/// Case: the user scrolls through history while a program is
/// reading stdin.
#[test]
fn scroll_writes_nothing_through_the_pty_writer() {
    let (mut term, sink) = detached_term();
    term.vt.scroll_moves = true;
    term.scroll(Scroll::Delta(3));
    term.scroll(Scroll::Bottom);
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}

/// Asserts that a selection operation the VT reports as a change arms
/// the coalescer, and one it reports as unchanged does not.
///
/// Case: the user starts a selection on an idle shell, then the host
/// re-sends a request the VT treats as a no-op.
#[test]
fn selection_operations_arm_the_coalescer_only_on_a_change() {
    let (mut term, _sink) = detached_term();
    let cell = GridPoint {
        line: GridLine(0),
        column: GridColumn(0),
    };
    term.vt.selection_changes = false;
    term.start_selection(cell, CellSide::Left, SelectionKind::Simple);
    term.extend_selection(cell, CellSide::Right);
    term.clear_selection();
    assert!(!term.coalescer.is_armed());

    term.vt.selection_changes = true;
    term.start_selection(cell, CellSide::Left, SelectionKind::Simple);
    assert!(term.coalescer.is_armed());
}
