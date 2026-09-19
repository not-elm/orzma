//! Tests for DECRQM and the DECRPM replies it draws.

use super::*;

/// The DECRPM reply to a DEC private DECRQM for `mode`, after `setup`.
fn private_report(setup: &[u8], mode: u32) -> Vec<u8> {
    let mut session = Session::new();
    session.feed(setup);
    session.feed(format!("\x1b[?{mode}$p").as_bytes()).replies
}

/// The DECRPM reply the device's power-up state gives for `mode`.
fn fresh_report(mode: u32) -> Vec<u8> {
    private_report(b"", mode)
}

/// Asserts that each flag-shaped private mode reports set after a
/// DECSET and reset after a DECRST.
///
/// Case: nvim asks at startup which modes the terminal already has
/// in force.
#[test]
fn flag_modes_report_their_state() {
    for mode in [1, 6, 7, 12, 25, 66, 1004, 1007, 2004, 2026] {
        let set = format!("\x1b[?{mode}h");
        let reset = format!("\x1b[?{mode}l");
        assert_eq!(
            private_report(set.as_bytes(), mode),
            format!("\x1b[?{mode};1$y").into_bytes(),
            "mode {mode} set"
        );
        assert_eq!(
            private_report(reset.as_bytes(), mode),
            format!("\x1b[?{mode};2$y").into_bytes(),
            "mode {mode} reset"
        );
    }
}

/// Asserts that the modes whose power-up state is set report set on a
/// fresh device, and that synchronized output reports reset.
///
/// Case: tmux probes a freshly spawned pane for synchronized output
/// before it draws anything.
#[test]
fn a_fresh_device_reports_its_power_up_state() {
    assert_eq!(fresh_report(7), b"\x1b[?7;1$y");
    assert_eq!(fresh_report(25), b"\x1b[?25;1$y");
    assert_eq!(fresh_report(1007), b"\x1b[?1007;1$y");
    assert_eq!(fresh_report(2026), b"\x1b[?2026;2$y");
}

/// Asserts that the three alternate-screen numbers all report which
/// screen is shown.
///
/// Case: a full-screen application entered the alternate screen with
/// `DECSET 1049` and a library underneath it asks about 47.
#[test]
fn the_alternate_screen_numbers_report_the_shown_screen() {
    for mode in [47, 1047, 1049] {
        assert_eq!(
            private_report(b"\x1b[?1049h", mode),
            format!("\x1b[?{mode};1$y").into_bytes()
        );
        assert_eq!(fresh_report(mode), format!("\x1b[?{mode};2$y").into_bytes());
    }
}

/// Asserts that mode 6 reports the origin mode of the screen on show.
///
/// Case: an application sets origin mode on the primary screen and
/// then enters the alternate screen, which never had it set.
#[test]
fn the_origin_mode_reports_the_screen_on_show() {
    assert_eq!(private_report(b"\x1b[?6h\x1b[?1049h", 6), b"\x1b[?6;2$y");
}

/// Asserts that only the mouse tracking level in force reports set,
/// and that resetting a level not in force changes nothing.
///
/// Case: an application upgrades click tracking to drag tracking and
/// a library then resets the click level it had set earlier.
#[test]
fn only_the_mouse_level_in_force_reports_set() {
    let setup = b"\x1b[?1000h\x1b[?1002h";
    assert_eq!(private_report(setup, 1000), b"\x1b[?1000;2$y");
    assert_eq!(private_report(setup, 1002), b"\x1b[?1002;1$y");
    let setup = b"\x1b[?1000h\x1b[?1002h\x1b[?1000l";
    assert_eq!(private_report(setup, 1002), b"\x1b[?1002;1$y");
}

/// Asserts that SGR mouse encoding reports its state and survives the
/// reset of an encoding that is not in force.
///
/// Case: an application enables SGR reports and then resets the UTF-8
/// encoding it never enabled.
#[test]
fn the_sgr_encoding_reports_its_state() {
    assert_eq!(fresh_report(1006), b"\x1b[?1006;2$y");
    assert_eq!(
        private_report(b"\x1b[?1006h\x1b[?1005l", 1006),
        b"\x1b[?1006;1$y"
    );
}

/// Asserts that mode 12 reports the blink DECSCUSR selected.
///
/// Case: an application selects a blinking bar cursor with DECSCUSR
/// and then asks whether the cursor blinks.
#[test]
fn the_blink_mode_reports_what_decscusr_selected() {
    assert_eq!(private_report(b"\x1b[5 q", 12), b"\x1b[?12;1$y");
    assert_eq!(private_report(b"\x1b[6 q", 12), b"\x1b[?12;2$y");
}

/// Asserts that a mode this terminal keeps no state for reports not
/// recognized.
///
/// Case: an application probes for column mode, the meta key, the
/// UTF-8 mouse encoding, and a mode number nothing assigns.
#[test]
fn stateless_and_unknown_modes_report_not_recognized() {
    for mode in [3, 40, 95, 1005, 1034, 1048, 9999] {
        assert_eq!(
            fresh_report(mode),
            format!("\x1b[?{mode};0$y").into_bytes(),
            "mode {mode}"
        );
    }
}

/// Asserts that an omitted parameter is answered as mode zero, not
/// recognized.
///
/// Case: a malformed DECRQM arrives with no mode number.
#[test]
fn an_omitted_parameter_is_answered_as_mode_zero() {
    assert_eq!(replies_of(b"\x1b[?$p"), b"\x1b[?0;0$y");
    assert_eq!(replies_of(b"\x1b[$p"), b"\x1b[0;0$y");
}

/// Asserts that the reply echoes a mode number too large for the
/// lookup exactly as it was sent.
///
/// Case: an application probes for a mode number above 65535 and
/// matches the reply by that number.
#[test]
fn a_large_mode_number_is_echoed_as_sent() {
    assert_eq!(fresh_report(737_769), b"\x1b[?737769;0$y");
}

/// Asserts that the ANSI form reports insert mode and nothing else.
///
/// Case: vttest walks the ANSI modes with DECRQM.
#[test]
fn the_ansi_form_reports_insert_mode() {
    assert_eq!(replies_of(b"\x1b[4$p"), b"\x1b[4;2$y");
    assert_eq!(replies_of(b"\x1b[4h\x1b[4$p"), b"\x1b[4;1$y");
    assert_eq!(replies_of(b"\x1b[20$p"), b"\x1b[20;0$y");
}

/// Asserts that the ANSI form does not read the private mode table.
///
/// Case: an application sends `CSI 7 $ p` without the private marker.
#[test]
fn the_ansi_form_does_not_report_private_modes() {
    assert_eq!(replies_of(b"\x1b[7$p"), b"\x1b[7;0$y");
}

/// Asserts that a DECRQM raises no chunk liveness.
///
/// Case: nvim probes a dozen modes at startup on an idle screen.
#[test]
fn a_mode_request_stages_no_damage() {
    assert!(!damage_of(b"\x1b[?2026$p"));
}

/// Asserts that every private mode this terminal keeps observable state
/// for is recognized by DECRQM, so a mode the write side starts honoring
/// cannot keep answering that it is not recognized.
///
/// Case: a later change teaches `DECSET` a mode number this terminal
/// used to ignore, and an application probes that number with DECRQM
/// before it relies on the mode.
#[test]
fn every_stateful_private_mode_is_reported() {
    for mode in 0..=2100u32 {
        for direction in ['h', 'l'] {
            let mut session = Session::new();
            let modes_before = session.0.device.modes();
            let origin_before = session.0.device.active_screen().origin_mode();
            session.feed_all(format!("\x1b[?{mode}{direction}").as_bytes());
            let changed = session.0.device.modes() != modes_before
                || session.0.device.active_screen().origin_mode() != origin_before;
            if changed {
                assert_ne!(
                    fresh_report(mode),
                    format!("\x1b[?{mode};0$y").into_bytes(),
                    "CSI ? {mode} {direction} changes state, so DECRQM must recognize it"
                );
            }
        }
    }
}
