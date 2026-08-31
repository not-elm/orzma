//! Tests for the device attribute requests and the replies they send
//! back to the host.

use super::*;

/// Asserts that a primary device attributes request reports the
/// terminal's architectural class.
///
/// Case: an application probes the terminal's capabilities at
/// startup and blocks until the class arrives.
#[test]
fn a_primary_attributes_request_reports_the_terminal_class() {
    assert_eq!(replies_of(b"\x1b[c"), b"\x1b[?6c");
}

/// Asserts that an explicit zero requests the same class an omitted
/// parameter does.
///
/// Case: a program that builds its sequences from an unset integer
/// variable emits `CSI 0 c`.
#[test]
fn an_explicit_zero_requests_the_same_class() {
    assert_eq!(replies_of(b"\x1b[0c"), b"\x1b[?6c");
}

/// Asserts that a nonzero parameter requests nothing.
///
/// Case: an application sends a device attributes variant this
/// terminal does not answer.
#[test]
fn a_nonzero_parameter_requests_nothing() {
    assert!(replies_of(b"\x1b[1c").is_empty());
}

/// Asserts that the seven-bit identify reports the primary class.
///
/// Case: an application written for a VT100 probes the terminal
/// with the obsolete `ESC Z` spelling.
#[test]
fn the_seven_bit_identify_reports_the_primary_class() {
    assert_eq!(replies_of(b"\x1bZ"), b"\x1b[?6c");
}

/// Asserts that the raw C1 byte for DECID reports the primary
/// class.
///
/// Case: a program emits an eight-bit identify on a terminal not
/// running in UTF-8 mode.
#[test]
fn the_raw_c1_identify_reports_the_primary_class() {
    assert_eq!(replies_of(b"\x9a"), b"\x1b[?6c");
}

/// Asserts that the UTF-8 encoding of U+009A reaches the same arm.
///
/// Case: a program running on a UTF-8 stream emits the identify.
#[test]
fn the_utf8_form_identifies_the_terminal() {
    assert_eq!(replies_of(b"\xc2\x9a"), b"\x1b[?6c");
}

/// Asserts that a secondary device attributes request reports the
/// terminal type and a firmware level.
///
/// Case: an application checks which terminal it is talking to
/// before enabling a version-gated workaround.
#[test]
fn a_secondary_attributes_request_reports_the_firmware_level() {
    let reply = replies_of(b"\x1b[>c");
    let version = reply
        .strip_prefix(b"\x1b[>0;".as_slice())
        .and_then(|rest| rest.strip_suffix(b";1c".as_slice()))
        .expect("the reply is framed as CSI > 0 ; Pv ; 1 c");
    assert!(version.iter().all(u8::is_ascii_digit));
    assert!(version.iter().any(|digit| *digit != b'0'));
}

/// Asserts that an explicit zero requests the same secondary
/// attributes an omitted parameter does.
///
/// Case: a program that builds its sequences from an unset integer
/// variable emits `CSI > 0 c`.
#[test]
fn an_explicit_zero_requests_the_same_secondary_attributes() {
    let reply = replies_of(b"\x1b[>0c");
    assert!(!reply.is_empty());
    assert_eq!(reply, replies_of(b"\x1b[>c"));
}

/// Asserts that a nonzero parameter requests no secondary
/// attributes.
///
/// Case: an application sends a secondary attributes variant this
/// terminal does not answer.
#[test]
fn a_nonzero_parameter_requests_no_secondary_attributes() {
    assert!(replies_of(b"\x1b[>1c").is_empty());
}

/// Asserts that a tertiary device attributes request is ignored.
///
/// Case: an application asks for the terminal unit id, which this
/// terminal never reports, and falls back after its own timeout.
#[test]
fn a_tertiary_attributes_request_is_ignored() {
    assert!(replies_of(b"\x1b[=c").is_empty());
}

/// Asserts that a version packs one hundred per component.
///
/// Case: a release bumps the minor version and the firmware level
/// DA2 reports has to move with it.
#[test]
fn a_version_packs_one_hundred_per_component() {
    assert_eq!(pack_version(1, 2, 3), 10_203);
}
