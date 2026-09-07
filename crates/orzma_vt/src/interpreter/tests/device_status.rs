//! Tests for the device status requests (`CSI 5 n`, `CSI 6 n`) and the
//! reports they send back to the host.

use super::*;

/// Asserts that a device status request reports the terminal as
/// operating normally.
///
/// Case: an application checks the link is alive before drawing.
#[test]
fn a_device_status_request_reports_ok() {
    assert_eq!(replies_of(b"\x1b[5n"), b"\x1b[0n");
}

/// Asserts that a cursor position request on a fresh terminal reports
/// the 1-based home position.
///
/// Case: ConPTY opens a pseudoconsole with an inherited cursor and
/// waits for the report before it starts the shell.
#[test]
fn a_cursor_position_request_reports_the_home_position() {
    assert_eq!(replies_of(b"\x1b[6n"), b"\x1b[1;1R");
}

/// Asserts that a cursor position request reports the cursor's row
/// and column as 1-based coordinates.
///
/// Case: a line editor addresses the cursor and asks where it landed
/// to learn the terminal's width.
#[test]
fn a_cursor_position_request_reports_one_based_coordinates() {
    let mut session = Session::new();
    session.feed(b"\x1b[2;3H");
    assert_eq!(session.feed(b"\x1b[6n").replies, b"\x1b[2;3R");
}

/// Asserts that in origin mode the reported row is relative to the top
/// margin rather than to the screen.
///
/// Case: a full-screen program confines the cursor to a scroll region
/// and asks for its position inside that region.
#[test]
fn a_cursor_position_request_is_relative_to_the_top_margin_in_origin_mode() {
    let mut session = Session::new();
    session.feed(b"\x1b[2;3r\x1b[?6h");
    assert_eq!(session.feed(b"\x1b[6n").replies, b"\x1b[1;1R");
    session.feed(b"\x1b[2;1H");
    assert_eq!(session.feed(b"\x1b[6n").replies, b"\x1b[2;1R");
}

/// Asserts that a status request this terminal does not implement
/// reports nothing.
///
/// Case: an application sends a printer or locator status request.
#[test]
fn an_unknown_status_request_reports_nothing() {
    assert!(replies_of(b"\x1b[7n").is_empty());
}
