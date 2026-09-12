//! Tests for the cursor presentation an application selects: the blink
//! `DECSET 12` toggles and the shape and blink `DECSCUSR` picks
//! together.

use super::*;
use crate::device::modes::CursorShape;

/// Whether the device's frame-ready cursor blinks.
fn cursor_blinking(device: &DeviceState) -> bool {
    device.cursor().blinking
}

/// The shape the device's frame-ready cursor carries.
fn cursor_shape(device: &DeviceState) -> CursorShape {
    device.cursor().shape
}

/// Asserts that a terminal that has seen no blink control reports a
/// steady block, which is the reset state of `DECSET 12`.
///
/// Case: a shell prints its first prompt on a freshly spawned terminal.
#[test]
fn a_fresh_terminal_reports_a_steady_block() {
    let device = interpret(b"$ ");
    assert!(!cursor_blinking(&device));
    assert_eq!(cursor_shape(&device), CursorShape::Block);
}

/// Asserts that `CSI ? 12 h` makes the cursor blink.
///
/// Case: the terminfo `cvvis` string asks for the very visible cursor
/// a pager uses to mark the read position.
#[test]
fn a_blink_mode_set_makes_the_cursor_blink() {
    let device = interpret(b"\x1b[?12h");
    assert!(cursor_blinking(&device));
}

/// Asserts that `CSI ? 12 l` returns a blinking cursor to steady.
///
/// Case: the terminfo `cnorm` string, which spells the normal cursor
/// `\E[?12l\E[?25h`, runs when a full-screen application exits.
#[test]
fn a_blink_mode_reset_returns_the_cursor_to_steady() {
    let mut session = Session::new();

    session.feed(b"\x1b[?12h");
    assert!(session.cursor_blinking());

    session.feed(b"\x1b[?12l");
    assert!(!session.cursor_blinking());
}

/// Asserts that `? 12` is applied when it shares a list with other
/// modes.
///
/// Case: the terminfo `cvvis` string sends `\E[?12;25h`, turning the
/// blink and the caret on in one sequence.
#[test]
fn a_blink_mode_set_inside_a_multi_mode_list_is_applied() {
    let device = interpret(b"\x1b[?25l\x1b[?12;25h");
    assert!(cursor_blinking(&device));
    assert!(device.cursor().visible);
}

/// Asserts that the ANSI-mode spelling `CSI 12 h`, which carries no
/// `?`, leaves the cursor steady.
///
/// Case: a program drives the terminal through non-private SM and RM,
/// and one of the numbers in its parameter list happens to be 12.
#[test]
fn an_ansi_mode_set_of_twelve_does_not_make_the_cursor_blink() {
    let device = interpret(b"\x1b[12h");
    assert!(!cursor_blinking(&device));
}

/// Asserts that the two blink modes reserved for a user preference,
/// `? 13` and `? 14`, are ignored in both directions.
///
/// Case: an application probes the terminal by driving the whole
/// documented blink family, not just the one mode it can set.
#[test]
fn the_user_preference_blink_modes_are_ignored() {
    for chunk in [b"\x1b[?13h".as_slice(), b"\x1b[?14h".as_slice()] {
        let device = interpret(chunk);
        assert!(
            !cursor_blinking(&device),
            "chunk {chunk:?} must not start the blink"
        );
    }

    for chunk in [b"\x1b[?13l".as_slice(), b"\x1b[?14l".as_slice()] {
        let mut session = Session::new();
        session.feed(b"\x1b[?12h");
        session.feed(chunk);
        assert!(
            session.cursor_blinking(),
            "chunk {chunk:?} must not stop the blink"
        );
    }
}

/// Asserts that a cursor checkpoint leaves the blink alone, so `DECRC`
/// does not restore the blink in force when `DECSC` ran.
///
/// Case: a program saves the cursor, turns the blink on to draw
/// attention to a prompt, and restores the cursor afterwards.
#[test]
fn a_cursor_checkpoint_leaves_the_blink_alone() {
    let device = interpret(b"\x1b7\x1b[?12h\x1b8");
    assert!(cursor_blinking(&device));
}

/// Asserts that a reset to initial state returns the cursor to steady.
///
/// Case: a script that left the blink on ends, and the shell runs
/// `tput reset`, whose `rs1` string is `\Ec`.
#[test]
fn a_reset_to_initial_state_returns_the_cursor_to_steady() {
    let device = interpret(b"\x1b[?12h\x1bc");
    assert!(!cursor_blinking(&device));
}

/// Asserts that the blink survives a round trip through the alternate
/// screen: a change made while there is still in force after the flip
/// back to the primary screen.
///
/// Case: a full-screen editor opens the alternate screen, turns the
/// blink on while it draws, and exits back to the shell.
#[test]
fn the_alternate_screen_keeps_the_blink() {
    let mut session = Session::new();

    session.feed(b"\x1b[?1049h\x1b[?12h");
    assert!(session.cursor_blinking());

    session.feed(b"\x1b[?1049l");
    assert!(session.cursor_blinking());
}

/// Asserts that a chunk turning the blink on is reported as
/// frame-relevant.
///
/// Case: a pager sends nothing but `CSI ? 12 h` between two repaints.
#[test]
fn turning_the_blink_on_makes_the_chunk_frame_relevant() {
    assert!(damage_of(b"\x1b[?12h"));
}

/// Asserts that a blink write that changes nothing leaves the chunk out
/// of the frame-relevant set.
///
/// Case: an application re-asserts the blink it already has as part of
/// the escape sequence it emits on every redraw.
#[test]
fn a_blink_write_that_changes_nothing_does_not_make_the_chunk_live() {
    assert!(!liveness_after(b"\x1b[?12h", b"\x1b[?12h"));
}

/// Asserts that the frame a chunk emits carries the blinking cursor.
///
/// Case: a pager turns the blink on, and the renderer must be told
/// before it draws the next frame.
#[test]
fn an_emitted_frame_carries_the_blinking_cursor() {
    let mut session = Session::new();
    session.feed(b"$ ");
    session
        .frame()
        .expect("the first chunk emits the bootstrap frame");

    session.feed(b"\x1b[?12h");
    let blinking = session.frame().expect("turning the blink on owes a frame");
    assert!(blinking.cursor.blinking);
}

/// Asserts that each `DECSCUSR` parameter reaches the device through
/// the parser.
///
/// Case: nvim switches the caret per mode as the user moves between
/// normal and insert mode.
#[test]
fn each_decscusr_parameter_reaches_the_device() {
    let expected = [
        (b"\x1b[2 q".as_slice(), CursorShape::Block, false),
        (b"\x1b[3 q".as_slice(), CursorShape::Underline, true),
        (b"\x1b[4 q".as_slice(), CursorShape::Underline, false),
        (b"\x1b[5 q".as_slice(), CursorShape::Bar, true),
        (b"\x1b[6 q".as_slice(), CursorShape::Bar, false),
    ];
    for (chunk, shape, blinking) in expected {
        let device = interpret(chunk);
        assert_eq!(cursor_shape(&device), shape, "chunk {chunk:?}");
        assert_eq!(cursor_blinking(&device), blinking, "chunk {chunk:?}");
    }
}

/// Asserts that an omitted parameter, a zero, and a one all reach the
/// device as the blinking block.
///
/// Case: a prompt framework sends the bare `CSI SP q`, and vim sends
/// `CSI 0 SP q` when it restores the caret on exit.
#[test]
fn an_omitted_zero_and_one_reach_the_device_as_the_blinking_block() {
    for chunk in [
        b"\x1b[ q".as_slice(),
        b"\x1b[0 q".as_slice(),
        b"\x1b[1 q".as_slice(),
    ] {
        let device = interpret(chunk);
        assert_eq!(cursor_shape(&device), CursorShape::Block, "chunk {chunk:?}");
        assert!(cursor_blinking(&device), "chunk {chunk:?}");
    }
}

/// Asserts that `CSI 7 SP q` restores the power-up style through the
/// parser.
///
/// Case: an application that took a blinking bar hands the terminal
/// back to the shell as it exits.
#[test]
fn a_decscusr_seven_restores_the_power_up_style() {
    let device = interpret(b"\x1b[5 q\x1b[7 q");
    assert_eq!(cursor_shape(&device), CursorShape::Block);
    assert!(!cursor_blinking(&device));
}

/// Asserts that a `DECSCUSR` parameter this terminal assigns no style
/// to leaves the cursor alone, and that the parser returns to ground so
/// the text after it still prints.
///
/// Case: a program written for a terminal with a longer style table
/// sends a parameter past this one's, and keeps writing.
#[test]
fn an_unassigned_decscusr_parameter_changes_nothing_and_the_text_prints() {
    let mut session = Session::new();
    session.feed(b"\x1b[5 q");
    session.feed(b"\x1b[99 qa");

    assert_eq!(session.cursor_shape(), CursorShape::Bar);
    assert!(session.cursor_blinking());
    assert_eq!(session.char_at(0, 0), 'a');
}

/// Asserts that a `SP q` carrying a private marker is not read as
/// `DECSCUSR`.
///
/// Case: a program mistakenly prefixes the sequence with `?`, the way
/// the private modes beside it are spelled.
#[test]
fn a_private_marker_does_not_reach_decscusr() {
    let device = interpret(b"\x1b[?5 q");
    assert_eq!(cursor_shape(&device), CursorShape::Block);
    assert!(!cursor_blinking(&device));
}

/// Asserts that the control functions sharing `DECSCUSR`'s intermediate
/// or its final byte do not reach `DECSCUSR`.
///
/// Case: a program drives SL, SR, DECSWBV, DECLL, and DECSCA, none of
/// which this terminal answers.
#[test]
fn the_neighbours_of_decscusr_stay_unanswered() {
    for chunk in [
        b"\x1b[5 @".as_slice(),
        b"\x1b[5 A".as_slice(),
        b"\x1b[5 t".as_slice(),
        b"\x1b[1q".as_slice(),
        b"\x1b[1\"q".as_slice(),
    ] {
        let device = interpret(chunk);
        assert_eq!(cursor_shape(&device), CursorShape::Block, "chunk {chunk:?}");
        assert!(!cursor_blinking(&device), "chunk {chunk:?}");
    }
}

/// Asserts that `DECSCUSR` reads only its first parameter slot.
///
/// Case: a program pads the sequence with a second parameter copied
/// from a different control function's spelling.
#[test]
fn decscusr_reads_only_its_first_slot() {
    let device = interpret(b"\x1b[5;2 q");
    assert_eq!(cursor_shape(&device), CursorShape::Bar);
    assert!(cursor_blinking(&device));
}

/// Asserts that a `DECSCUSR` whose first slot carries subparameters is
/// read from that slot's first integer rather than being rejected.
///
/// Case: a program spells the parameter with a colon, the way the
/// direct-colour SGR forms beside it are spelled.
#[test]
fn a_colon_in_the_first_slot_does_not_reject_decscusr() {
    let device = interpret(b"\x1b[5:2 q");
    assert_eq!(cursor_shape(&device), CursorShape::Bar);
    assert!(cursor_blinking(&device));
}

/// Asserts that `DECSET 12` and `DECSCUSR` write one shared blink, the
/// last writer winning, and that turning the blink off leaves the shape
/// `DECSCUSR` chose in place.
///
/// Case: nvim picks a blinking bar for insert mode, and the terminfo
/// `cnorm` string later turns the blink off without naming a shape.
#[test]
fn the_blink_mode_and_decscusr_share_one_state() {
    let mut session = Session::new();

    session.feed(b"\x1b[5 q");
    assert!(session.cursor_blinking());

    session.feed(b"\x1b[?12l");
    assert!(!session.cursor_blinking());
    assert_eq!(session.cursor_shape(), CursorShape::Bar);

    session.feed(b"\x1b[?12h");
    assert!(session.cursor_blinking());

    session.feed(b"\x1b[2 q");
    assert!(!session.cursor_blinking());
    assert_eq!(session.cursor_shape(), CursorShape::Block);
}

/// Asserts that the shape is not carried by a cursor checkpoint,
/// survives a round trip through the alternate screen, and returns to
/// the power-up block on a reset to initial state.
///
/// Case: a full-screen editor picks a bar, saves and restores the
/// cursor while it draws, opens and leaves the alternate screen, and
/// the shell later runs `tput reset`.
#[test]
fn the_shape_follows_the_device_mode_lifetime() {
    let mut session = Session::new();

    session.feed(b"\x1b[5 q\x1b7\x1b[2 q\x1b8");
    assert_eq!(session.cursor_shape(), CursorShape::Block);

    session.feed(b"\x1b[3 q\x1b[?1049h\x1b[?1049l");
    assert_eq!(session.cursor_shape(), CursorShape::Underline);

    session.feed(b"\x1bc");
    assert_eq!(session.cursor_shape(), CursorShape::Block);
}

/// Asserts that a shape change is frame-relevant and reaches an emitted
/// frame, while a `DECSCUSR` that changes nothing does not make the
/// chunk live.
///
/// Case: nvim sends its per-mode caret sequence on every mode change,
/// including the ones that re-assert the caret already in force.
#[test]
fn a_shape_change_reaches_a_frame_and_a_repeat_does_not() {
    assert!(damage_of(b"\x1b[5 q"));
    assert!(!liveness_after(b"\x1b[5 q", b"\x1b[5 q"));

    let mut session = Session::new();
    session.feed(b"$ ");
    session
        .frame()
        .expect("the first chunk emits the bootstrap frame");

    session.feed(b"\x1b[5 q");
    let frame = session.frame().expect("a shape change owes a frame");
    assert_eq!(frame.cursor.shape, CursorShape::Bar);
    assert!(frame.cursor.blinking);
}

/// Asserts that `DECSCUSR` leaves the cursor's visibility alone, so a
/// caret hidden by DECTCEM stays hidden while its shape changes.
///
/// Case: a full-screen editor hides the caret for a repaint and picks
/// the shape it wants before showing it again.
#[test]
fn decscusr_leaves_the_cursor_visibility_alone() {
    let device = interpret(b"\x1b[?25l\x1b[5 q");
    assert!(!device.cursor().visible);
    assert_eq!(cursor_shape(&device), CursorShape::Bar);
}
