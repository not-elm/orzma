//! Tests for the window title: the operating system commands that
//! set it and the stack that saves and restores it.

use super::*;

/// Asserts that an OS command sets the window title and reports it.
///
/// Case: a shell prompt sets the title before printing.
#[test]
fn a_title_sequence_reports_the_new_title() {
    let (_device, output) = interpret_fully(b"\x1b]0;hi\x07");
    assert_eq!(output.signals, vec![VtSignal::Title("hi".to_owned())]);
}

/// Asserts that setting a title leaves the chunk undamaged.
///
/// Case: a prompt sets the title without printing anything.
#[test]
fn a_title_sequence_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]0;hi\x07"));
}

/// Asserts that a saved title is restored by a pop.
///
/// Case: a full-screen editor saves the shell's title, sets its
/// own, and restores it on the way out.
#[test]
fn a_popped_title_is_restored() {
    let (device, output) = interpret_fully(b"\x1b]0;shell\x07\x1b[22t\x1b]0;editor\x07\x1b[23t");
    assert_eq!(
        output.signals,
        vec![
            VtSignal::Title("shell".to_owned()),
            VtSignal::Title("editor".to_owned()),
            VtSignal::Title("shell".to_owned()),
        ]
    );
    assert_eq!(device.title(), Some("shell"));
}

/// Asserts that popping an empty stack reports nothing.
///
/// Case: a program restores a title it never saved.
#[test]
fn popping_an_empty_title_stack_reports_nothing() {
    let (_device, output) = interpret_fully(b"\x1b[23t");
    assert!(output.signals.is_empty());
}

/// Asserts that popping a title saved before any was set reports a
/// reset rather than an empty title.
///
/// Case: a program saves the title at startup, sets its own, and
/// restores on exit, with the shell having set none.
#[test]
fn popping_an_unset_title_reports_a_reset() {
    let (device, output) = interpret_fully(b"\x1b[22t\x1b]0;editor\x07\x1b[23t");
    assert_eq!(
        output.signals,
        vec![VtSignal::Title("editor".to_owned()), VtSignal::ResetTitle]
    );
    assert_eq!(device.title(), None);
}

/// Asserts that a reset returns the title to its default and says
/// so.
///
/// Case: the user runs `reset` after a program left a title behind.
#[test]
fn a_reset_reports_the_title_returning_to_its_default() {
    let (_device, output) = interpret_fully(b"\x1b]0;hi\x07\x1bc");
    assert_eq!(
        output.signals,
        vec![VtSignal::Title("hi".to_owned()), VtSignal::ResetTitle]
    );
}

/// Asserts that a reset with no title set reports nothing.
///
/// Case: the user runs `reset` twice in a row.
#[test]
fn a_reset_without_a_title_reports_nothing() {
    let (_device, output) = interpret_fully(b"\x1bc");
    assert!(output.signals.is_empty());
}

/// Asserts that a title terminated by ST sets the window title just
/// as one terminated by BEL does.
///
/// Case: a program that emits the seven-bit string terminator sets
/// the window title.
#[test]
fn a_string_terminator_ends_a_title_too() {
    let (_device, output) = interpret_fully(b"\x1b]0;hi\x1b\\");
    assert_eq!(output.signals, vec![VtSignal::Title("hi".to_owned())]);
}

/// Asserts that a window operation carrying a private marker does
/// not reach the title stack.
///
/// Case: an application sends `CSI > 22 t` to set the title
/// modifier, which this terminal does not implement.
#[test]
fn a_private_window_operation_does_not_reach_the_title_stack() {
    let (_device, output) = interpret_fully(b"\x1b]0;hi\x07\x1b[>22t\x1b[23t");
    assert_eq!(output.signals, vec![VtSignal::Title("hi".to_owned())]);
}

/// Asserts that an operating system command is not fatal, and that
/// the parser returns to ground behind it.
///
/// Case: a shell prompt sets the window title and goes on printing
/// behind it.
#[test]
fn a_title_sequence_is_ignored_rather_than_fatal() {
    let device = interpret(b"\x1b]0;hi\x07a");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}
