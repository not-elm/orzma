//! Tests for synchronized output (`DECSET 2026` / `DECRST 2026`): the
//! mode itself and where an interpret call stops.

use super::*;

/// Asserts that `DECSET 2026` opens a synchronized update and `DECRST
/// 2026` closes it.
///
/// Case: fzf wraps one redraw of its list in a synchronized update.
#[test]
fn the_mode_follows_decset_and_decrst() {
    let device = interpret(b"\x1b[?2026h");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Active
    );
    let device = interpret(b"\x1b[?2026h\x1b[?2026l");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Inactive
    );
}

/// Asserts that a repeated `DECSET 2026` leaves an open update open,
/// and that a `DECRST 2026` with none open changes nothing.
///
/// Case: an application sends `DECSET 2026` twice before it draws, and
/// another resets the mode on its way out without having set it.
#[test]
fn a_repeated_open_and_a_stray_close_change_nothing() {
    let device = interpret(b"\x1b[?2026h\x1b[?2026h");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Active
    );
    let device = interpret(b"\x1b[?2026l");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Inactive
    );
}

/// Asserts that the eight-bit CSI opens a synchronized update as the
/// seven-bit one does.
///
/// Case: an application running with eight-bit controls wraps its
/// redraw in a synchronized update.
#[test]
fn the_eight_bit_csi_opens_an_update() {
    let device = interpret(b"\x9b?2026h");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Active
    );
}

/// Asserts that opening or closing a synchronized update raises no
/// chunk liveness.
///
/// Case: an application opens an update and then stalls before it
/// draws anything.
#[test]
fn opening_and_closing_an_update_stages_no_damage() {
    assert!(!damage_of(b"\x1b[?2026h"));
    assert!(!liveness_after(b"\x1b[?2026h", b"\x1b[?2026l"));
}

/// Asserts that a hard reset closes an open synchronized update.
///
/// Case: an application dies inside an update and the user runs
/// `reset` to take the terminal back.
#[test]
fn a_hard_reset_closes_an_open_update() {
    let device = interpret(b"\x1b[?2026h\x1bc");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Inactive
    );
}

/// Asserts that a soft reset leaves an open synchronized update open.
///
/// Case: an application sends `DECSTR` while it redraws inside an
/// update.
#[test]
fn a_soft_reset_leaves_an_open_update_open() {
    let device = interpret(b"\x1b[?2026h\x1b[!p");
    assert_eq!(
        device.modes().synchronized_output,
        SynchronizedOutput::Active
    );
}

/// Asserts that an interpret call stops right behind the sequence that
/// closes a synchronized update and reports how far it got.
///
/// Case: an application ends one redraw and starts the next in a
/// single write, so both land in one PTY chunk.
#[test]
fn an_interpret_call_stops_where_an_update_closes() {
    let mut session = Session::new();
    session.feed(b"\x1b[?2026h");
    let chunk = b"a\x1b[?2026lb";
    let output = session.0.interpret(chunk);
    assert_eq!(output.consumed, chunk.len() - 1);
    assert!(output.synchronized_update_closed);
    assert_eq!(session.char_at(0, 0), 'a');
    assert_eq!(session.char_at(0, 1), ' ');
}

/// Asserts that the eight-bit CSI closes an update and stops the call
/// as the seven-bit one does.
///
/// Case: an application running with eight-bit controls ends one
/// redraw and starts the next in a single write.
#[test]
fn the_eight_bit_csi_closes_an_update_and_stops_the_call() {
    let mut session = Session::new();
    session.feed(b"\x9b?2026h");
    let chunk = b"a\x9b?2026lb";
    let output = session.0.interpret(chunk);
    assert_eq!(output.consumed, chunk.len() - 1);
    assert!(output.synchronized_update_closed);
}

/// Asserts that a close on the last byte of a chunk is still reported
/// as a close.
///
/// Case: an application ends its redraw and then waits for input, so
/// the close is the last thing in the PTY chunk.
#[test]
fn a_close_at_the_end_of_a_chunk_is_reported() {
    let mut session = Session::new();
    session.feed(b"\x1b[?2026h");
    let chunk = b"a\x1b[?2026l";
    let output = session.0.interpret(chunk);
    assert_eq!(output.consumed, chunk.len());
    assert!(output.synchronized_update_closed);
}

/// Asserts that a chunk with no synchronized update is consumed whole
/// and reports no close.
///
/// Case: the shell prints a command's plain output.
#[test]
fn a_chunk_without_an_update_is_consumed_whole() {
    let mut session = Session::new();
    let output = session.0.interpret(b"abc");
    assert_eq!(output.consumed, 3);
    assert!(!output.synchronized_update_closed);
}

/// Asserts that a reset of mode 2026 with no update open stops
/// nothing.
///
/// Case: an application resets every mode it might have set on its
/// way out, synchronized output included.
#[test]
fn a_stray_reset_does_not_stop_the_call() {
    let mut session = Session::new();
    let chunk = b"\x1b[?2026lab";
    let output = session.0.interpret(chunk);
    assert_eq!(output.consumed, chunk.len());
    assert!(!output.synchronized_update_closed);
}

/// Asserts that every mode of the closing sequence is applied before
/// the call stops.
///
/// Case: an application closes its update and hides the cursor in one
/// `CSI ? 2026 ; 25 l`.
#[test]
fn the_closing_sequence_applies_all_of_its_modes() {
    let mut session = Session::new();
    session.feed(b"\x1b[?2026h");
    let chunk = b"\x1b[?2026;25lx";
    let output = session.0.interpret(chunk);
    assert!(output.synchronized_update_closed);
    assert_eq!(output.consumed, chunk.len() - 1);
    assert!(!session.cursor_visible());
}

/// Asserts that a hard reset inside an open update stops the call as
/// a close does.
///
/// Case: the user runs `reset` while a dead application's update is
/// still open, and the shell prompt follows in the same chunk.
#[test]
fn a_hard_reset_inside_an_update_stops_the_call() {
    let mut session = Session::new();
    session.feed(b"\x1b[?2026h");
    let chunk = b"\x1bc$ ";
    let output = session.0.interpret(chunk);
    assert_eq!(output.consumed, 2);
    assert!(output.synchronized_update_closed);
}

/// Asserts that resubmitting the unconsumed rest interprets the whole
/// chunk.
///
/// Case: the PTY owner feeds the rest of a chunk after it took the
/// frame a closed update made ready.
#[test]
fn resubmitting_the_rest_interprets_the_whole_chunk() {
    let mut session = Session::new();
    session.feed(b"\x1b[?2026h");
    let output = session.feed_all(b"a\x1b[?2026lb");
    assert!(output.synchronized_update_closed);
    assert_eq!(output.consumed, 10);
    assert_eq!(session.char_at(0, 1), 'b');
}
