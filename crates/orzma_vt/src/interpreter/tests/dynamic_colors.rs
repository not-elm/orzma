//! Tests for the dynamic colors: the operating system commands that
//! recolor the default foreground, the default background, and the
//! text cursor, report them, and return them to their defaults.

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

/// Asserts that a query leaves the chunk undamaged, a reply owing no
/// repaint of its own.
///
/// Case: nvim probes the background at startup without recoloring it,
/// and the terminal must not open a coalesce window for a frame that
/// carries nothing new.
#[test]
fn a_query_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]11;?\x07"));
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
    let (device, output) = interpret_fully(b"\x1b]10;rgb:ff/00/00;?\x07");
    assert_eq!(output.replies, b"\x1b]11;rgb:0000/0000/0000\x07");
    assert_eq!(
        device.palette().foreground,
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0x00
        }
    );
}

/// Asserts that an `OSC 12` writes the colour it carries to the
/// palette's cursor color, and that an `OSC 112` returns it to unset.
///
/// Case: nvim recolors the cursor while it runs and restores it on
/// exit.
#[test]
fn an_osc_12_sets_the_cursor_color_and_an_osc_112_unsets_it() {
    let device = interpret(b"\x1b]12;rgb:ff/88/00\x07");
    assert_eq!(
        device.palette().cursor,
        Some(Rgb {
            r: 0xff,
            g: 0x88,
            b: 0x00
        })
    );
    let device = interpret(b"\x1b]12;rgb:ff/88/00\x07\x1b]112\x07");
    assert_eq!(device.palette().cursor, None);
}

/// Asserts that a query of an unset cursor color is answered with the
/// default foreground rather than left unanswered, following a
/// foreground an `OSC 10` recolored.
///
/// Case: a program asks for the cursor colour at startup, before
/// anything has set one, and waits for the reply.
#[test]
fn a_query_of_an_unset_cursor_color_is_answered_with_the_foreground() {
    assert_eq!(
        replies_of(b"\x1b]12;?\x07"),
        b"\x1b]12;rgb:ffff/ffff/ffff\x07"
    );
    assert_eq!(
        replies_of(b"\x1b]10;rgb:12/34/56\x07\x1b]12;?\x07"),
        b"\x1b]12;rgb:1212/3434/5656\x07"
    );
}

/// Asserts that a query of a set cursor color is answered with that
/// colour, closed the way the query was.
///
/// Case: a terminfo-driven program sets the cursor colour and reads it
/// back, closing both commands with `ESC \`.
#[test]
fn a_query_of_a_set_cursor_color_is_answered_with_that_color() {
    assert_eq!(
        replies_of(b"\x1b]12;#ff8800\x1b\\\x1b]12;?\x1b\\"),
        b"\x1b]12;rgb:ffff/8888/0000\x1b\\"
    );
}

/// Asserts that a chained query starting at `OSC 10` is answered for
/// the cursor color too, the third reply naming `12`.
///
/// Case: a program saves the text, ground, and cursor colours with a
/// single command before it recolors them.
#[test]
fn a_chained_query_reaches_the_cursor_color() {
    assert_eq!(
        replies_of(b"\x1b]10;?;?;?\x07"),
        b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0000/0000/0000\x07\x1b]12;rgb:ffff/ffff/ffff\x07"
    );
}

/// Asserts that a cursor recolor marks its chunk damaged and owes a
/// frame that carries the palette and repaints no row.
///
/// Case: nvim enters insert mode and recolors the cursor while the
/// text on screen stays as it is.
#[test]
fn a_cursor_recolor_owes_a_frame_that_repaints_no_row() {
    let mut session = Session::new();
    let _ = session.frame();
    assert!(session.feed(b"\x1b]12;rgb:ff/88/00\x07").damaged);
    let frame = session.frame().expect("a cursor recolor owes a frame");
    assert!(frame.rows.is_empty());
    assert_eq!(
        frame
            .palette
            .expect("a cursor recolor owes the frame its palette")
            .cursor,
        Some(Rgb {
            r: 0xff,
            g: 0x88,
            b: 0x00
        })
    );
}

/// Asserts that a cursor recolor to the colour already held, and a
/// reset of an unset cursor color, leave the chunk undamaged.
///
/// Case: nvim re-sends the same cursor colour on every mode change, and
/// a program sends `OSC 112` on exit without having set a colour.
#[test]
fn a_cursor_recolor_that_changes_nothing_leaves_the_chunk_undamaged() {
    let mut session = Session::new();
    assert!(session.feed(b"\x1b]12;rgb:ff/88/00\x07").damaged);
    assert!(!session.feed(b"\x1b]12;rgb:ff/88/00\x07").damaged);
    assert!(!damage_of(b"\x1b]112\x07"));
}

/// Asserts that a command recoloring the text, the ground, and the
/// cursor owes one frame that repaints every row and carries all three
/// colours, and nothing after it.
///
/// Case: a theme script written for xterm recolors all three in a
/// single `OSC 10`.
#[test]
fn a_three_color_chain_owes_one_full_frame() {
    let mut session = Session::new();
    let _ = session.frame();
    assert!(
        session
            .feed(b"\x1b]10;rgb:11/11/11;rgb:22/22/22;rgb:33/33/33\x07")
            .damaged
    );
    let frame = session.frame().expect("a recolor owes a frame");
    assert_eq!(frame.rows.len(), 3);
    let palette = frame.palette.expect("a recolor owes the frame its palette");
    assert_eq!(
        palette.foreground,
        Rgb {
            r: 0x11,
            g: 0x11,
            b: 0x11
        }
    );
    assert_eq!(
        palette.background,
        Rgb {
            r: 0x22,
            g: 0x22,
            b: 0x22
        }
    );
    assert_eq!(
        palette.cursor,
        Some(Rgb {
            r: 0x33,
            g: 0x33,
            b: 0x33
        })
    );
    assert!(session.frame().is_none());
}
