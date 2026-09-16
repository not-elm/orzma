//! Tests for `set_focused` and focus reporting (DECSET 1004).

use super::*;

/// Asserts that each focus transition writes its report while the
/// application has focus reporting enabled.
///
/// Case: the user returns to nvim and then switches away again.
#[test]
fn set_focused_reports_each_transition_while_focus_reporting_is_enabled() {
    let (mut term, sink) = detached_term();
    term.vt.modes.focus_in_out = true;
    term.set_focused(true).expect("set_focused");
    term.set_focused(false).expect("set_focused");
    assert_eq!(sink.contents(), b"\x1b[I\x1b[O");
}

/// Asserts that repeating the current focus state writes nothing.
///
/// Case: the window blurs right after the pane was deactivated, so
/// the host reports the loss twice.
#[test]
fn set_focused_writes_nothing_when_the_state_is_unchanged() {
    let (mut term, sink) = detached_term();
    term.vt.modes.focus_in_out = true;
    term.set_focused(true).expect("set_focused");
    term.set_focused(false).expect("set_focused");
    term.set_focused(false).expect("set_focused");
    assert_eq!(sink.contents(), b"\x1b[I\x1b[O");
}

/// Asserts that a change made while focus reporting is off is recorded
/// but not reported, so enabling focus reporting reports nothing until
/// the next change.
///
/// Case: a program enables focus reporting at start-up in a pane that
/// already has focus.
#[test]
fn enabling_focus_reporting_reports_nothing_until_the_next_change() {
    let (mut term, sink) = detached_term();
    term.set_focused(true).expect("set_focused");
    term.vt.modes.focus_in_out = true;
    term.set_focused(true).expect("set_focused");
    assert_eq!(sink.contents(), b"");
    term.set_focused(false).expect("set_focused");
    assert_eq!(sink.contents(), b"\x1b[O");
}

/// Asserts that a focus report leaves a scrolled-back viewport where it
/// is and schedules no repaint.
///
/// Case: the user is reading scrollback and switches applications.
#[test]
fn set_focused_does_not_snap_a_scrolled_back_viewport() {
    let (mut term, sink) = detached_term();
    term.vt.modes.focus_in_out = true;
    term.vt.display_offset = DisplayOffset(3);
    term.set_focused(true).expect("set_focused");
    assert_eq!(sink.contents(), b"\x1b[I");
    assert_eq!(term.vt.display_offset, DisplayOffset(3));
    assert!(!term.vt.scrolls.iter().any(|s| matches!(s, Scroll::Bottom)));
    assert!(!term.coalescer.is_armed());
}

/// Asserts that a failed focus write still records the new state, so
/// repeating that state attempts no second write.
///
/// Case: the window regains focus while the pane's PTY rejects writes,
/// and the user then resizes the window before focus changes again.
#[test]
fn a_failed_focus_write_keeps_the_new_state() {
    let mut term = OrzmaTty::detached(
        FakeVt::new(grid(80, 24)),
        grid(80, 24),
        Box::new(FailingSink),
    )
    .expect("OrzmaTty::detached");
    term.vt.modes.focus_in_out = true;
    assert!(matches!(
        term.set_focused(true),
        Err(OrzmaTtyError::PtyWrite(_))
    ));
    term.set_focused(true)
        .expect("an unchanged state attempts no write, so it cannot fail");
}
