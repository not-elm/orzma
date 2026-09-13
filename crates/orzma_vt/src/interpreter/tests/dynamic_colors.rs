//! Tests for the dynamic colors: the operating system commands that
//! recolor the default foreground and background, report them, and
//! return them to their defaults.

use super::*;
use crate::device::color::Palette;

/// Asserts that an `OSC 11` writes the colour it carries to the
/// palette's background.
///
/// Case: a colour-scheme script gives the terminal a dark grey ground
/// before the user's prompt draws.
#[test]
fn an_osc_11_set_reaches_the_palette_background() {
    let device = interpret(b"\x1b]11;rgb:20/20/20\x07");
    assert_eq!(
        device.palette().background,
        Rgb {
            r: 0x20,
            g: 0x20,
            b: 0x20
        }
    );
}

/// Asserts that an `OSC 10` writes the colour it carries to the
/// palette's foreground.
///
/// Case: a colour-scheme script recolors the terminal's text.
#[test]
fn an_osc_10_set_reaches_the_palette_foreground() {
    let device = interpret(b"\x1b]10;rgb:12/34/56\x07");
    assert_eq!(
        device.palette().foreground,
        Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
}

/// Asserts that a recolor marks its chunk damaged and hands the new
/// palette to the frame.
///
/// Case: a theme script gives the terminal a dark grey ground, and
/// every cell drawn with the default background has to be repainted
/// against it.
#[test]
fn a_recolor_marks_the_chunk_damaged_and_carries_the_palette() {
    let mut session = Session::new();
    assert!(session.feed(b"\x1b]11;rgb:20/20/20\x07").damaged);
    let frame = session.frame().expect("a recolor owes a frame");
    assert_eq!(
        frame
            .palette
            .expect("a recolor owes the frame its palette")
            .background,
        Rgb {
            r: 0x20,
            g: 0x20,
            b: 0x20
        }
    );
}

/// Asserts that a recolor to the colour the palette already holds
/// leaves the chunk undamaged rather than repainting.
///
/// Case: a shell prompt re-applies the same theme before every command
/// it runs.
#[test]
fn a_recolor_to_the_current_color_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]11;rgb:00/00/00\x07"));
}

/// Asserts that a query is answered with the colour the palette holds
/// at that point.
///
/// Case: nvim asks for the background at startup so that it can decide
/// whether to set its `background` option to dark or light.
#[test]
fn a_query_is_answered_with_the_color_the_palette_holds() {
    assert_eq!(
        replies_of(b"\x1b]11;?\x07"),
        b"\x1b]11;rgb:0000/0000/0000\x07"
    );
}

/// Asserts that a chained query is answered once per color, each reply
/// naming its own colour number, in the order the query asked.
///
/// Case: a program saves both the text and the ground colour with a
/// single command before it recolors them.
#[test]
fn a_chained_query_is_answered_once_per_color() {
    assert_eq!(
        replies_of(b"\x1b]10;?;?\x07"),
        b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0000/0000/0000\x07"
    );
}

/// Asserts that an `OSC 110` returns the foreground to its default and
/// leaves the background alone.
///
/// Case: a program restores the text colour it changed before it exits,
/// while the ground colour a theme script set stays as it is.
#[test]
fn an_osc_110_restores_the_default_foreground() {
    let device = interpret(b"\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:20/20/20\x07\x1b]110\x07");
    assert_eq!(device.palette().foreground, Palette::default().foreground);
    assert_eq!(
        device.palette().background,
        Rgb {
            r: 0x20,
            g: 0x20,
            b: 0x20
        }
    );
}

/// Asserts that an `OSC 111` returns the background to its default and
/// leaves the foreground alone.
///
/// Case: a program restores the ground colour it changed before it
/// exits, while the text colour a theme script set stays as it is.
#[test]
fn an_osc_111_restores_the_default_background() {
    let device = interpret(b"\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:20/20/20\x07\x1b]111\x07");
    assert_eq!(device.palette().background, Palette::default().background);
    assert_eq!(
        device.palette().foreground,
        Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
}

/// Asserts that a set and a query in one command apply in the order
/// they appear.
///
/// Case: a theme script recolors the text and asks for the ground
/// colour in the same command, so that it can restore the ground on
/// exit.
#[test]
fn a_set_and_a_query_in_one_command_apply_in_order() {
    let mut session = Session::new();
    let output = session.feed(b"\x1b]10;rgb:ff/00/00;?\x07");
    assert_eq!(output.replies, b"\x1b]11;rgb:0000/0000/0000\x07");
    assert_eq!(
        session.0.device.palette().foreground,
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0x00
        }
    );
}
