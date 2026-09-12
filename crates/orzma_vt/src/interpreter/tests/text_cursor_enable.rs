//! Tests for DECTCEM, the private mode that decides whether the text
//! cursor is drawn.

use super::*;

/// The visibility the device's frame-ready cursor carries.
fn cursor_visible(device: &DeviceState) -> bool {
    device.cursor().visible
}

/// Asserts that `CSI ? 25 l` makes the cursor invisible.
///
/// Case: nvim hides the caret before it repaints a pane.
#[test]
fn a_dectcem_reset_hides_the_cursor() {
    let device = interpret(b"\x1b[?25l");
    assert!(!cursor_visible(&device));
}

/// Asserts that `CSI ? 25 h` makes a hidden cursor visible again.
///
/// Case: nvim finishes the repaint and brings the caret back.
#[test]
fn a_dectcem_set_shows_a_hidden_cursor() {
    let mut session = Session::new();

    session.feed(b"\x1b[?25l");
    assert!(!session.cursor_visible());

    session.feed(b"\x1b[?25h");
    assert!(session.cursor_visible());
}

/// Asserts that a terminal that has seen no DECTCEM reports a visible
/// cursor.
///
/// Case: a shell prints its prompt on a freshly spawned terminal.
#[test]
fn a_fresh_terminal_reports_a_visible_cursor() {
    let device = interpret(b"$ ");
    assert!(cursor_visible(&device));
}

/// Asserts that `? 25` inside a multi-mode list is applied, and that an
/// unimplemented number in the list does not stop a later implemented
/// one from being applied.
///
/// Case: an application turns cursor visibility off together with focus
/// reporting and a mode this terminal does not implement, then the
/// terminfo `cvvis` string turns the caret back on beside the blink.
#[test]
fn a_dectcem_reset_inside_a_multi_mode_list_is_applied() {
    let device = interpret(b"\x1b[?1004h\x1b[?25;9999;1004l");
    assert!(!cursor_visible(&device));
    assert!(!device.modes().focus_in_out);

    let through_cvvis = interpret(b"\x1b[?25l\x1b[?12;25h");
    assert!(cursor_visible(&through_cvvis));
}

/// Asserts that the ANSI-mode spelling `CSI 25 l`, which carries no
/// `?`, leaves the cursor visible.
///
/// Case: a program drives the terminal through non-private SM and RM,
/// and one of the numbers in its parameter list happens to be 25.
#[test]
fn an_ansi_mode_reset_of_twenty_five_does_not_hide_the_cursor() {
    let device = interpret(b"\x1b[25l");
    assert!(cursor_visible(&device));
}

/// Asserts that DECSC and DECRC neither save nor restore cursor
/// visibility, in the `CSI ? 1048` spelling and the `ESC 7` / `ESC 8`
/// one alike.
///
/// Case: a full-screen application checkpoints and restores the cursor
/// around a redraw, sometimes with the caret hidden when it saves and
/// sometimes when it restores, and reaches the checkpoint through the
/// private-mode spelling on one path and `ESC 7` / `ESC 8` on another.
#[test]
fn a_cursor_checkpoint_leaves_cursor_visibility_alone() {
    let restored_while_visible = interpret(b"\x1b[?25l\x1b[?1048h\x1b[?25h\x1b[?1048l");
    assert!(cursor_visible(&restored_while_visible));

    let restored_while_hidden = interpret(b"\x1b[?1048h\x1b[?25l\x1b[?1048l");
    assert!(!cursor_visible(&restored_while_hidden));

    let restored_through_esc = interpret(b"\x1b[?25l\x1b7\x1b[?25h\x1b8");
    assert!(cursor_visible(&restored_through_esc));
}

/// Asserts that `RIS` returns DECTCEM to its visible default.
///
/// Case: a program leaves the caret hidden when it dies, and the shell
/// issues a hard reset to get a usable terminal back.
#[test]
fn a_reset_to_initial_state_restores_the_visible_cursor() {
    let device = interpret(b"\x1b[?25l\x1bc");
    assert!(cursor_visible(&device));
}

/// Asserts that switching to the alternate screen and back keeps the
/// DECTCEM state the application set, through either alternate-screen
/// entry sequence.
///
/// Case: nvim hides the caret before it enters the alternate screen at
/// startup, and the caret stays hidden through the alternate screen and
/// after nvim returns to the primary screen when it exits.
#[test]
fn the_alternate_screen_keeps_the_cursor_visibility() {
    let device = interpret(b"\x1b[?25l\x1b[?1049h");
    assert!(!cursor_visible(&device));

    let via_1047 = interpret(b"\x1b[?25l\x1b[?1047h");
    assert!(!cursor_visible(&via_1047));

    let mut session = Session::new();
    session.feed(b"\x1b[?25l\x1b[?1049h");
    session.feed(b"\x1b[?1049l");
    assert!(!session.cursor_visible());
}

/// Asserts that a chunk hiding the cursor is reported as
/// frame-relevant.
///
/// Case: nvim sends nothing but `CSI ? 25 l` between two repaints.
#[test]
fn hiding_the_cursor_makes_the_chunk_frame_relevant() {
    assert!(damage_of(b"\x1b[?25l"));
}

/// Asserts that a DECTCEM write that changes nothing leaves the chunk
/// out of the frame-relevant set.
///
/// Case: an application re-asserts the cursor visibility it already has
/// as part of the escape sequence it emits on every redraw.
#[test]
fn a_dectcem_write_that_changes_nothing_does_not_make_the_chunk_live() {
    assert!(!liveness_after(b"\x1b[?25l", b"\x1b[?25l"));
}

/// Asserts that the frame a chunk emits carries the hidden cursor, and
/// that a later re-show reaches an emitted frame the same way.
///
/// Case: nvim hides the caret before a repaint, then finishes the
/// repaint and shows the caret again.
#[test]
fn an_emitted_frame_carries_the_hidden_cursor() {
    let mut session = Session::new();
    session.feed(b"$ ");
    session
        .frame()
        .expect("the first chunk emits the bootstrap frame");

    session.feed(b"\x1b[?25l");
    let hidden = session.frame().expect("hiding the cursor owes a frame");
    assert!(!hidden.cursor.visible);

    session.feed(b"\x1b[?25h");
    let shown = session.frame().expect("showing the cursor owes a frame");
    assert!(shown.cursor.visible);
}
