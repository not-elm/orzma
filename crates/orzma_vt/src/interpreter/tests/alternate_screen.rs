//! Tests for the private modes that show the alternate screen, and for
//! how they interact over the cursor checkpoint and the erase they imply.

use super::*;

/// Asserts that `?47h` shows the alternate screen and repaints the
/// whole viewport, leaving the primary screen's contents in place
/// behind it.
///
/// Case: a program built against the old termcap pair opens on the
/// alternate screen while the shell's prompt sits on the primary.
#[test]
fn decset_47_shows_the_alternate_screen() {
    let mut session = Session::new();
    session.feed(b"a");
    session.frame();
    let output = session.feed(b"\x1b[?47h");
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
    assert!(output.damaged);
    assert_eq!(session.char_at(0, 0), ' ');
    let frame = session.frame().expect("a flip emits a full frame");
    assert_eq!(frame.rows.len(), 3);
}

/// Asserts that `?47l` returns to the primary screen with its
/// contents intact, repaints the whole viewport, and raises no
/// eviction when the alternate screen held no placements.
///
/// Case: the program exits and the shell's prompt from before it
/// must reappear.
#[test]
fn decrst_47_returns_to_the_primary_screen() {
    let mut session = Session::new();
    session.feed(b"a\x1b[?47h");
    session.frame();
    let output = session.feed(b"\x1b[?47l");
    assert_eq!(session.active_screen(), ScreenKind::Primary);
    assert!(output.damaged);
    assert!(output.signals.is_empty());
    assert_eq!(session.char_at(0, 0), 'a');
    let frame = session.frame().expect("the flip back emits a full frame");
    assert_eq!(frame.rows.len(), 3);
}

/// Asserts that a DECSET already on the alternate screen and a
/// DECRST already on the primary screen are complete no-ops rather
/// than repaints.
///
/// Case: a wrapper script runs a program's `rmcup` string although
/// the program never got to send `smcup`, and later a program
/// re-sends its initialisation string while already full-screen.
#[test]
fn a_redundant_alternate_screen_switch_does_nothing() {
    let mut session = Session::new();
    let output = session.feed(b"\x1b[?47l");
    assert!(!output.damaged);
    assert!(output.signals.is_empty());
    assert_eq!(session.active_screen(), ScreenKind::Primary);

    session.feed(b"\x1b[?47h");
    let output = session.feed(b"\x1b[?47h");
    assert!(!output.damaged);
    assert!(output.signals.is_empty());
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
}

/// Asserts that leaving the alternate screen names its placements
/// in the chunk's own signals and leaves the primary screen's
/// placement in the next frame.
///
/// Case: a full-screen program that mounted a webview exits, and
/// the shell's own webview from before it must survive.
#[test]
fn leaving_the_alternate_screen_evicts_only_its_placements() {
    let mut session = Session::new();
    let kept = InstanceId(1);
    session.mount(kept);
    session.frame();
    session.feed(b"\x1b[?47h");
    let dropped = InstanceId(2);
    session.mount(dropped);
    session.frame();
    let output = session.feed(b"\x1b[?47l");
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewEvicted {
            placements: vec![dropped]
        }]
    );
    let frame = session.frame().expect("the flip back emits");
    let listed: Vec<InstanceId> = frame
        .placements
        .expect("a placement change is listed")
        .iter()
        .map(|placement| placement.id)
        .collect();
    assert_eq!(listed, vec![kept]);
}

/// Asserts that `?1049h` shows the alternate screen erased, whatever
/// the previous full-screen program left on it.
///
/// Case: vim starts after a program that used the bare `?47` pair
/// exited with its last frame still on the alternate screen.
#[test]
fn decset_1049_erases_the_alternate_screen() {
    let mut session = Session::new();
    session.feed(b"\x1b[?47hx\x1b[?47l");
    let output = session.feed(b"\x1b[?1049h");
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
    assert!(output.damaged);
    assert_eq!(session.char_at(0, 0), ' ');
}

/// Asserts that `?1049l` leaves the alternate screen's contents in
/// place rather than erasing them on the way out.
///
/// Case: vim exits, and a later program enters with the bare `?47h`
/// and finds vim's last frame still there.
#[test]
fn decrst_1049_does_not_erase_the_alternate_screen() {
    let mut session = Session::new();
    session.feed(b"\x1b[?1049hx\x1b[?1049l");
    session.feed(b"\x1b[?47h");
    assert_eq!(session.char_at(0, 0), 'x');
}

/// Asserts that `?1049h` saves the cursor into the primary screen's
/// own DECSC slot, so a later `ESC 8` on the primary finds the
/// position the flip saved rather than the shell's earlier save.
///
/// Case: a shell saves its cursor with `ESC 7`, runs a full-screen
/// program whose `?1049h` overwrites that save, and restores with
/// `ESC 8` after the program exits.
#[test]
fn decset_1049_saves_the_cursor_into_the_primary_checkpoint() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b7\x1b[1;4H\x1b[?1049h\x1b[?1049l\x1b8");
    assert_eq!(session.cursor_column(), 3);
}

/// Asserts that `?1049l` restores the cursor `?1049h` saved, moving
/// it from wherever the primary screen's cursor was left in between.
///
/// Case: a program enters with `?1049h`, drops back to the primary
/// screen with `?47l` to print a line, returns with `?47h`, and
/// finally exits with `?1049l`.
#[test]
fn decrst_1049_restores_the_saved_primary_cursor() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b[?1049h\x1b[?47l\x1b[1;4H\x1b[?47h");
    let output = session.feed(b"\x1b[?1049l");
    assert_eq!(session.active_screen(), ScreenKind::Primary);
    assert!(output.damaged);
    assert_eq!(session.cursor_column(), 1);
}

/// Asserts that `?1049h` while already on the alternate screen does
/// nothing: no erase, no repaint, no signal, and the alternate
/// screen's own DECSC slot left alone.
///
/// Case: a program re-sends its terminal initialisation string
/// while it is already running full-screen.
#[test]
fn a_redundant_decset_1049_leaves_the_alternate_checkpoint_alone() {
    let mut session = Session::new();
    session.feed(b"\x1b[?1049hx\x1b[1;2H\x1b7\x1b[1;4H");
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
    let output = session.feed(b"\x1b[?1049h");
    assert!(!output.damaged);
    assert!(output.signals.is_empty());
    assert_eq!(session.char_at(0, 0), 'x');
    session.feed(b"\x1b8");
    assert_eq!(session.cursor_column(), 1);
}

/// Asserts that `?1049l` while already on the primary screen does
/// nothing, not even the DECRC.
///
/// Case: a wrapper script runs a program's `rmcup` string although
/// the program was killed before it sent `smcup`.
#[test]
fn a_redundant_decrst_1049_does_not_restore_the_cursor() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b7\x1b[1;4H");
    let output = session.feed(b"\x1b[?1049l");
    assert!(!output.damaged);
    assert!(output.signals.is_empty());
    assert_eq!(session.cursor_column(), 3);
}

/// Asserts that `?1049l` after a bare `?47h` restores whatever the
/// primary screen's DECSC slot holds, which is the home position
/// when nothing was ever saved.
///
/// Case: a program enters with the old `?47h` but exits with the
/// terminfo `?1049l`.
#[test]
fn decrst_1049_after_decset_47_restores_the_existing_checkpoint() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;4H\x1b[?47h\x1b[?1049l");
    assert_eq!(session.cursor_column(), 0);
}

/// Asserts that a `?47l` between `?1049h` and `?1049l` leaves the
/// saved cursor unrestored.
///
/// Case: a program enters with `?1049h`, leaves with the bare
/// `?47l`, and its `rmcup` string sends `?1049l` afterwards.
#[test]
fn decrst_47_then_decrst_1049_never_restores_the_saved_cursor() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b[?1049h\x1b[?47l\x1b[1;4H\x1b[?1049l");
    assert_eq!(session.cursor_column(), 3);
}

/// Asserts that in `CSI ? 47;1049 h` the 47 enters first and the
/// 1049 then does nothing: no save into the alternate screen's DECSC
/// slot, no erase.
///
/// Case: a program lists both alternate-screen numbers in one
/// DECSET to satisfy old and new terminals at once.
#[test]
fn decset_47_and_1049_in_one_sequence_enters_without_saving() {
    let mut session = Session::new();
    session.feed(b"\x1b[?47hx\x1b[1;3H\x1b7\x1b[1;4H\x1b[?47l\x1b[1;2H\x1b7\x1b[1;4H");
    session.feed(b"\x1b[?47;1049h");
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
    assert_eq!(session.char_at(0, 0), 'x');
    session.feed(b"\x1b8");
    assert_eq!(session.cursor_column(), 2);
    session.feed(b"\x1b[?1049l");
    assert_eq!(session.cursor_column(), 1);
}

/// Asserts that in `CSI ? 1049;47 l` the 1049 flips back and
/// restores, and the 47 then does nothing.
///
/// Case: a program lists both alternate-screen numbers in one
/// DECRST on the way out.
#[test]
fn decrst_1049_and_47_in_one_sequence_restores_once() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b[?1049h\x1b[?47l\x1b[1;4H\x1b[?47h");
    let output = session.feed(b"\x1b[?1049;47l");
    assert_eq!(session.active_screen(), ScreenKind::Primary);
    assert!(output.damaged);
    assert_eq!(session.cursor_column(), 1);
}

/// Asserts that `?1047l` erases the alternate screen before
/// returning to the primary screen.
///
/// Case: a program that uses the 1047 pair exits, and the next
/// program to enter with the bare `?47h` finds a blank screen.
#[test]
fn decrst_1047_erases_the_alternate_screen() {
    let mut session = Session::new();
    session.feed(b"\x1b[?1047hx");
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
    let output = session.feed(b"\x1b[?1047l");
    assert_eq!(session.active_screen(), ScreenKind::Primary);
    assert!(output.damaged);
    session.feed(b"\x1b[?47h");
    assert_eq!(session.char_at(0, 0), ' ');
}

/// Asserts that `?1047h` neither saves the cursor nor erases the
/// alternate screen on the way in.
///
/// Case: a shell saves its cursor with `ESC 7`, runs a program that
/// enters with `?1047h` onto a screen a previous program left text
/// on, and restores with `ESC 8` after it exits.
#[test]
fn decset_1047_neither_saves_nor_erases() {
    let mut session = Session::new();
    session.feed(b"\x1b[?47hx\x1b[?47l\x1b[1;2H\x1b7\x1b[1;4H\x1b[?1047h");
    assert_eq!(session.char_at(0, 0), 'x');
    session.feed(b"\x1b[?1047l\x1b8");
    assert_eq!(session.cursor_column(), 1);
}

/// Asserts that `?1047l` while already on the primary screen erases
/// nothing and repaints nothing.
///
/// Case: a wrapper script runs a program's exit string although the
/// program never entered the alternate screen, while the shell's
/// output is on the primary.
#[test]
fn a_redundant_decrst_1047_does_not_erase_the_primary_screen() {
    let mut session = Session::new();
    session.feed(b"x");
    let output = session.feed(b"\x1b[?1047l");
    assert!(!output.damaged);
    assert!(output.signals.is_empty());
    assert_eq!(session.char_at(0, 0), 'x');
}
