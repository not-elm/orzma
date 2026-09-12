//! Tests for the indexed palette: the operating system commands that
//! recolor its slots, report them, and return them to their defaults.

use super::*;
use crate::device::color::Palette;

/// The xterm default a palette slot holds before any override.
fn default_slot(index: usize) -> Rgb {
    Palette::default().indexed[index]
}

/// The colour palette slot `index` of the session's device holds.
fn slot(session: &Session, index: usize) -> Rgb {
    session.0.device.palette().indexed[index]
}

/// Asserts that an `OSC 4` sets the palette slot it names.
///
/// Case: a theme script recolors ANSI red with `OSC 4 ; 1 ; rgb:…`
/// before the user runs `ls --color`.
#[test]
fn an_osc_4_recolors_the_slot_it_names() {
    let device = interpret(b"\x1b]4;1;rgb:12/34/56\x07");
    assert_eq!(
        device.palette().indexed[1],
        Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
}

/// Asserts that a colour number of 256 leaves slot 0 alone rather than
/// wrapping onto it.
///
/// Case: a script written for xterm sets the bold colour through
/// `OSC 4 ; 256 ; …`, the special-color spelling this terminal ignores.
#[test]
fn a_special_color_number_leaves_slot_zero_alone() {
    let device = interpret(b"\x1b]4;256;rgb:ff/ff/ff\x07");
    assert_eq!(device.palette().indexed[0], default_slot(0));
}

/// Asserts that a query closed by BEL is answered with the slot's
/// colour, closed by BEL as well.
///
/// Case: a program saves the user's red before recoloring it, and asks
/// with the BEL spelling most shell scripts use.
#[test]
fn a_query_closed_by_bel_is_answered_with_bel() {
    assert_eq!(
        replies_of(b"\x1b]4;1;?\x07"),
        b"\x1b]4;1;rgb:cdcd/0000/0000\x07"
    );
}

/// Asserts that a query closed by the seven-bit string terminator is
/// answered with it.
///
/// Case: a program built on terminfo closes its query with `ESC \`.
#[test]
fn a_query_closed_by_st_is_answered_with_st() {
    assert_eq!(
        replies_of(b"\x1b]4;1;?\x1b\\"),
        b"\x1b]4;1;rgb:cdcd/0000/0000\x1b\\"
    );
}

/// Asserts that a query closed by the eight-bit string terminator is
/// answered with the seven-bit one rather than echoing the C1 byte.
///
/// Case: a program on an eight-bit channel closes its query with a raw
/// `0x9C`.
#[test]
fn a_query_closed_by_the_eight_bit_st_is_answered_with_the_seven_bit_st() {
    assert_eq!(
        replies_of(b"\x1b]4;1;?\x9c"),
        b"\x1b]4;1;rgb:cdcd/0000/0000\x1b\\"
    );
}

/// Asserts that a terminator arriving in a later chunk still decides
/// how the reply is closed.
///
/// Case: the PTY splits a query so that its closing BEL lands in the
/// next read.
#[test]
fn a_terminator_in_the_next_chunk_still_closes_the_reply() {
    let mut session = Session::new();
    assert!(session.feed(b"\x1b]4;1;?").replies.is_empty());
    assert_eq!(
        session.feed(b"\x07").replies,
        b"\x1b]4;1;rgb:cdcd/0000/0000\x07"
    );
}

/// Asserts that a query after a set in the same command reports the
/// colour just set.
///
/// Case: a program sets a slot and asks for it back in one command to
/// confirm that the terminal took the change.
#[test]
fn a_query_after_a_set_in_one_command_reports_the_new_color() {
    assert_eq!(
        replies_of(b"\x1b]4;1;rgb:12/34/56;1;?\x07"),
        b"\x1b]4;1;rgb:1212/3434/5656\x07"
    );
}

/// Asserts that each query in one command gets a reply of its own, in
/// order.
///
/// Case: a program saves several slots by asking for all of them in
/// one command.
#[test]
fn each_query_in_one_command_gets_its_own_reply() {
    assert_eq!(
        replies_of(b"\x1b]4;0;?;1;?\x07"),
        b"\x1b]4;0;rgb:0000/0000/0000\x07\x1b]4;1;rgb:cdcd/0000/0000\x07"
    );
}

/// Asserts that a recolor marks its chunk damaged.
///
/// Case: a theme script recolors a slot the shell prompt shows, and
/// the owner must open a coalesce window for the frame that carries the
/// new palette.
#[test]
fn a_recolor_marks_the_chunk_damaged() {
    assert!(damage_of(b"\x1b]4;1;rgb:12/34/56\x07"));
}

/// Asserts that setting a slot to the colour it already holds leaves
/// the chunk undamaged rather than repainting.
///
/// Case: a theme script re-applies the stock xterm red on every
/// prompt.
#[test]
fn a_recolor_to_the_current_color_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]4;1;rgb:cd/00/00\x07"));
}

/// Asserts that a query leaves the chunk undamaged.
///
/// Case: a program probes the palette at startup without changing it.
#[test]
fn a_query_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]4;1;?\x07"));
}

/// Asserts that an `OSC 104` naming a slot returns it to its default,
/// marks the chunk damaged, and leaves the other slots alone.
///
/// Case: a program restores the one slot it recolored before it exits.
#[test]
fn an_osc_104_restores_the_slot_it_names() {
    let mut session = Session::new();
    session.feed(b"\x1b]4;1;rgb:12/34/56;2;rgb:12/34/56\x07");
    assert!(session.feed(b"\x1b]104;1\x07").damaged);
    assert_eq!(slot(&session, 1), default_slot(1));
    assert_eq!(
        slot(&session, 2),
        Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
}

/// Asserts that a bare `OSC 104` returns every slot to its default and
/// marks the chunk damaged.
///
/// Case: `tput init` on an ncurses 6.6 entry sends `oc`, a bare
/// `OSC 104`, after a theme script recolored several slots.
#[test]
fn a_bare_osc_104_restores_every_slot() {
    let mut session = Session::new();
    session.feed(b"\x1b]4;1;rgb:12/34/56;200;rgb:12/34/56\x07");
    assert!(session.feed(b"\x1b]104\x07").damaged);
    assert_eq!(slot(&session, 1), default_slot(1));
    assert_eq!(slot(&session, 200), default_slot(200));
}

/// Asserts that `ESC c` returns a recolored slot to its default and
/// marks the chunk damaged, even when nothing was ever printed.
///
/// Case: the user runs `reset` on a macOS terminfo entry whose `rs1` is
/// a bare `ESC c`, after a theme script recolored the palette.
#[test]
fn a_reset_restores_a_recolored_slot_and_repaints() {
    let mut session = Session::new();
    session.feed(b"\x1b]4;1;rgb:12/34/56\x07");
    assert!(session.feed(b"\x1bc").damaged);
    assert_eq!(slot(&session, 1), default_slot(1));
}
