//! Tests for the bounds the parser imposes on a sequence: how many
//! parameters survive, and what an intermediate byte disqualifies.

use super::*;

/// Asserts that a parameter list long enough to hit the parser's
/// own cap loses its tail, which this terminal cannot detect.
///
/// Case: an application sets nine attributes and two direct colours
/// in one sequence, and the background never arrives.
#[test]
fn a_parameter_list_past_the_parser_cap_loses_its_tail() {
    let device = interpret(b"\x1b[0;1;2;3;4;5;7;8;9;38;2;255;0;0;48;2;0;0;255mx");
    let cell = device.active_screen().viewport_row(ViewportLine(0))[0];
    assert_eq!(cell.fg, Color::Rgb(Rgb { r: 255, g: 0, b: 0 }));
    assert_eq!(cell.bg, Color::DefaultBackground);
}

/// Asserts that a sequence carrying an intermediate does not reach
/// the control function that shares its final byte.
///
/// Case: an application changes the attributes of a rectangle with
/// `CSI 1 ; 2 $ r`.
#[test]
fn an_intermediate_does_not_reach_the_scroll_region() {
    let device = interpret(b"\x1b[1;2$ra\n\nb");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(2))[1].c,
        'b'
    );
}

/// Asserts that a sequence whose intermediate falls out of the
/// parameter slice past the parser's parameter limit still does not
/// reach the control function that shares its final byte.
///
/// Case: an application changes the attributes of a rectangle with
/// `CSI 1 ; 2 $ r`, sent with a parameter list long enough to exhaust
/// the parser's 32-parameter limit before the trailing `$` arrives.
#[test]
fn a_truncated_intermediate_does_not_reach_the_scroll_region() {
    let chunk = format!("\x1b[1;2{}$ra\n\nb", ";".repeat(29));
    let device = interpret(chunk.as_bytes());
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(2))[1].c,
        'b'
    );
}
