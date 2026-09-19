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
