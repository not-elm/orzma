//! Tests for designating character sets into the G-sets and invoking
//! them, through both the locking and the single shifts.

use super::*;

/// Asserts that a set designated into G0 maps the characters
/// printed after it.
///
/// Case: a program draws a horizontal rule by designating DEC
/// Special Graphics into G0 and printing `q`.
#[test]
fn a_set_designated_into_g0_maps_what_follows() {
    let device = interpret(b"\x1b(0q");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        '─'
    );
}

/// Asserts that designating ASCII over a G code restores letters.
///
/// Case: a program finishes a box and emits `ESC ( B` before
/// printing more text.
#[test]
fn redesignating_ascii_restores_letters() {
    let device = interpret(b"\x1b(0\x1b(Bq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'q'
    );
}

/// Asserts that a final with no set behind it designates ASCII
/// rather than leaving the previous set in force.
///
/// Case: a program draws a box, then designates the Finnish
/// national replacement set with `ESC ( C` before writing a label.
#[test]
fn an_unsupported_designation_falls_back_to_ascii() {
    let device = interpret(b"\x1b(0\x1b(Cq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'q'
    );
}

/// Asserts that a designation whose final takes two bytes falls
/// back to ASCII just as a one-byte final does.
///
/// Case: a program draws a box with line drawing, then designates
/// the Greek supplemental set with `ESC ( " >` before writing a
/// label.
#[test]
fn a_two_byte_final_designation_falls_back_to_ascii() {
    let device = interpret(b"\x1b(0\x1b(\">q");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'q'
    );
}

/// Asserts that `SO` invokes G1 into GL and `SI` returns G0 to it.
///
/// Case: a program designates line drawing into G1 once, then
/// brackets each run of box characters with `SO` and `SI`.
#[test]
fn the_shift_out_and_shift_in_pair_swaps_the_invoked_set() {
    let device = interpret(b"\x1b)0\x0eq\x0fq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        '─'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        'q'
    );
}

/// Asserts that `ESC n` invokes G2 into GL for everything that
/// follows.
///
/// Case: a program parks line drawing in G2 and locks it into GL
/// once, leaving G0 free to hold the set its text is written in.
#[test]
fn the_locking_shift_two_invokes_g2() {
    let device = interpret(b"\x1b*0q\x1bnq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'q'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        '─'
    );
}

/// Asserts that `ESC o` invokes G3 into GL for everything that
/// follows.
///
/// Case: a program that already uses G2 parks a second set in G3 and
/// locks that one into GL instead.
#[test]
fn the_locking_shift_three_invokes_g3() {
    let device = interpret(b"\x1b+0q\x1boq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'q'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        '─'
    );
}

/// Asserts that the seven-bit `SS2` maps one character and then
/// stops applying.
///
/// Case: a program prints one box character mid-sentence with
/// `ESC N`.
#[test]
fn the_seven_bit_single_shift_two_lasts_one_character() {
    let device = interpret(b"\x1b*0\x1bNqq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        '─'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        'q'
    );
}

/// Asserts that the raw C1 byte for SS2 single-shifts too.
///
/// Case: a program emits an eight-bit single shift on a terminal not
/// running in UTF-8 mode.
#[test]
fn the_raw_c1_byte_single_shifts() {
    let device = interpret(b"\x1b*0\x8eqq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        '─'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        'q'
    );
}

/// Asserts that the UTF-8 encoding of U+008E single-shifts too.
///
/// Case: a program running on a UTF-8 stream emits the single shift.
#[test]
fn the_utf8_form_single_shifts() {
    let device = interpret(b"\x1b*0\xc2\x8eqq");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        '─'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[1].c,
        'q'
    );
}
