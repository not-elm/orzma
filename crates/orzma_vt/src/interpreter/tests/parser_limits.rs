//! Tests for the bounds the parser imposes on a sequence: how many
//! parameters survive, and what an intermediate byte disqualifies.

use super::*;
use crate::device::color::Palette;

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

/// Asserts that an `OSC 4` carrying more pairs than the parser's
/// parameter cap applies its first thirty-one pairs and loses the rest,
/// which this terminal cannot detect.
///
/// Case: a theme script recolors thirty-two slots in one command.
#[test]
fn an_osc_4_past_the_parser_cap_loses_its_thirty_second_pair() {
    let pairs: String = (0..32)
        .map(|index| format!(";{index};rgb:01/02/03"))
        .collect();
    let device = interpret(format!("\x1b]4{pairs}\x07").as_bytes());
    assert_eq!(device.palette().indexed[30], Rgb { r: 1, g: 2, b: 3 });
    assert_eq!(device.palette().indexed[31], Palette::default().indexed[31]);
}

/// Compares one interpreted chunk against a baseline across the modes,
/// the cursor, the palette, the title, every visible row's cells, and
/// the signals and reply bytes the chunk produced.
fn assert_same_observable_effect(
    device: &DeviceState,
    output: &InterpretOutput,
    baseline: &DeviceState,
    baseline_output: &InterpretOutput,
    spelling: &str,
) {
    assert_eq!(device.modes(), baseline.modes(), "{spelling}");
    assert_eq!(device.cursor(), baseline.cursor(), "{spelling}");
    assert_eq!(device.palette(), baseline.palette(), "{spelling}");
    assert_eq!(device.title(), baseline.title(), "{spelling}");
    assert_eq!(output.signals, baseline_output.signals, "{spelling}");
    assert_eq!(output.replies, baseline_output.replies, "{spelling}");
    for line in 0..device.active_screen().grid_size().rows {
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(line)),
            baseline.active_screen().viewport_row(ViewportLine(line)),
            "{spelling} row {line}"
        );
    }
}

/// Asserts that no CSI final byte, sent with a trailing intermediate
/// and with or without a private marker, reaches a control function
/// whose effect this terminal can observe.
///
/// Case: an application lays out a form with the DEC rectangle-editing
/// sequences `CSI Pt ; Pl ; Pb ; Pr $ r`, `$ t`, `$ v`, `$ x` and
/// `$ z`, each of which shares its final byte with a control function
/// this terminal does answer.
#[test]
fn an_intermediate_reaches_no_implemented_control_function() {
    const MARKERS: &[&str] = &["", "?", ">"];
    const PARAMETERS: &[&str] = &[
        "", "2", "2;3", "1;1", "4", "5", "6", "7", "22", "23", "25", "1049",
    ];
    const SEEDS: &[&str] = &["", "\x1b[4h"];
    const PROBE_SUFFIX: &str = "z\tx\x1b8y\x1b[23t";
    for seed in SEEDS {
        let prefix = format!(
            "\x1b]0;f\x07\x1b[22t\x1b]0;g\x07\x1b[22t\x1b]0;h\x07\x1b[3g{seed}ab\r\ncdef\x1b[6G\x1bH\x1b[2;3H"
        );
        let (baseline, baseline_output) =
            interpret_sized(20, format!("{prefix}{PROBE_SUFFIX}").as_bytes());
        for marker in MARKERS {
            for parameters in PARAMETERS {
                for final_byte in 0x40..=0x7Eu8 {
                    let sequence = format!("\x1b[{marker}{parameters}${}", final_byte as char);
                    let chunk = format!("{prefix}{sequence}{PROBE_SUFFIX}");
                    let (device, output) = interpret_sized(20, chunk.as_bytes());
                    let spelling = format!(
                        "CSI {marker}{parameters}${} after {seed:?}",
                        final_byte as char
                    );
                    assert_same_observable_effect(
                        &device,
                        &output,
                        &baseline,
                        &baseline_output,
                        &spelling,
                    );
                }
            }
        }
    }
}
