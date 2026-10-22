//! Tests for the kitty keyboard protocol's flag requests.

use super::*;

/// Reports the reply to a flag query sent right after `chunk`.
fn flags_after(chunk: &[u8]) -> Vec<u8> {
    replies_of(&[chunk, b"\x1b[?u"].concat())
}

/// Asserts that a fresh terminal reports no keyboard flags.
///
/// Case: an application queries the flags before it has requested any.
#[test]
fn a_fresh_terminal_reports_no_keyboard_flags() {
    assert_eq!(replies_of(b"\x1b[?u"), b"\x1b[?0u");
}

/// Asserts that a push makes the pushed flags the ones in force.
///
/// Case: an application enables disambiguation on startup.
#[test]
fn a_push_selects_the_pushed_flags() {
    assert_eq!(flags_after(b"\x1b[>1u"), b"\x1b[?1u");
}

/// Asserts that a push drops the flags this terminal does not implement.
///
/// Case: an application requests several enhancements at once, only one of which is implemented.
#[test]
fn unsupported_flags_are_dropped_from_a_push() {
    assert_eq!(flags_after(b"\x1b[>5u"), b"\x1b[?1u");
    assert_eq!(flags_after(b"\x1b[>30u"), b"\x1b[?0u");
}

/// Asserts that a flag parameter past the 16-bit range keeps its low bits instead of saturating
/// to the 16-bit maximum.
///
/// Case: a client sends a flag value past the 16-bit range.
#[test]
fn oversized_parameters_are_masked_rather_than_saturated() {
    assert_eq!(flags_after(b"\x1b[>65536u"), b"\x1b[?0u");
    assert_eq!(flags_after(b"\x1b[>65537u"), b"\x1b[?1u");
    assert_eq!(flags_after(b"\x1b[=4294967297;1u"), b"\x1b[?1u");
}

/// Asserts that a pop restores the entry beneath the popped one.
///
/// Case: an application pushes a temporary flag set and leaves it again.
#[test]
fn a_pop_restores_the_flags_below() {
    assert_eq!(flags_after(b"\x1b[>1u\x1b[>0u\x1b[<u"), b"\x1b[?1u");
}

/// Asserts that a pop of more entries than the stack holds clears every flag.
///
/// Case: an application pops more often than it pushed.
#[test]
fn popping_past_the_bottom_clears_every_flag() {
    assert_eq!(flags_after(b"\x1b[>1u\x1b[<5u"), b"\x1b[?0u");
    assert_eq!(flags_after(b"\x1b[>1u\x1b[<70000u"), b"\x1b[?0u");
    assert_eq!(flags_after(b"\x1b[<u"), b"\x1b[?0u");
}

/// Asserts that a pop of zero entries leaves the stack unchanged.
///
/// Case: an application sends a pop request with a zero count.
#[test]
fn a_pop_of_zero_entries_changes_nothing() {
    assert_eq!(flags_after(b"\x1b[>1u\x1b[<0u"), b"\x1b[?1u");
}

/// Asserts that a set request replaces, adds or removes bits according to its mode.
///
/// Case: an application adjusts the flags in force without pushing.
#[test]
fn a_set_replaces_adds_and_removes_bits() {
    assert_eq!(flags_after(b"\x1b[=1u"), b"\x1b[?1u");
    assert_eq!(flags_after(b"\x1b[=1u\x1b[=1;3u"), b"\x1b[?0u");
    assert_eq!(flags_after(b"\x1b[=1u\x1b[=1;3u\x1b[=1;2u"), b"\x1b[?1u");
}

/// Asserts that a set request with an unknown mode changes nothing.
///
/// Case: an application sends a mode this terminal does not define.
#[test]
fn a_set_with_an_unknown_mode_is_ignored() {
    assert_eq!(flags_after(b"\x1b[=1;4u"), b"\x1b[?0u");
    assert_eq!(flags_after(b"\x1b[>1u\x1b[=0;4u"), b"\x1b[?1u");
}

/// Asserts that a set on an empty stack creates an entry that a single pop removes.
///
/// Case: an application sets flags without pushing and later pops once.
#[test]
fn a_set_on_an_empty_stack_is_undone_by_one_pop() {
    assert_eq!(flags_after(b"\x1b[=1u\x1b[<u"), b"\x1b[?0u");
}

/// Asserts that each screen keeps its own flag stack.
///
/// Case: a shell with flags pushed runs a full-screen program that pushes its own flags on the
/// alternate screen, and the program is run again after it exits.
#[test]
fn each_screen_keeps_its_own_flags() {
    assert_eq!(flags_after(b"\x1b[>1u\x1b[?1049h"), b"\x1b[?0u");
    assert_eq!(
        flags_after(b"\x1b[>1u\x1b[?1049h\x1b[>0u\x1b[?1049l"),
        b"\x1b[?1u"
    );
    assert_eq!(
        flags_after(b"\x1b[?1049h\x1b[>1u\x1b[?1049l\x1b[?1049h"),
        b"\x1b[?1u"
    );
}

/// Asserts that leaving the alternate screen reports the main screen's flags.
///
/// Case: a full-screen program exits while its flags are still pushed.
#[test]
fn leaving_the_alternate_screen_hands_the_shell_its_legacy_keys() {
    assert_eq!(flags_after(b"\x1b[?1049h\x1b[>1u\x1b[?1049l"), b"\x1b[?0u");
}

/// Asserts that entering the alternate screen again does not clear its flag stack.
///
/// Case: a full-screen program is re-entered with mode 1047 after it pushed flags.
#[test]
fn the_alternate_screen_erase_keeps_its_flags() {
    assert_eq!(
        flags_after(b"\x1b[?1047h\x1b[>1u\x1b[?1047l\x1b[?1047h"),
        b"\x1b[?1u"
    );
}

/// Asserts that an erase and a cursor restore leave the flags alone.
///
/// Case: an application clears the screen and restores the cursor while its flags are pushed.
#[test]
fn an_erase_or_a_cursor_restore_leaves_the_flags_alone() {
    assert_eq!(flags_after(b"\x1b[>1u\x1b[2J"), b"\x1b[?1u");
    assert_eq!(flags_after(b"\x1b7\x1b[>1u\x1b8"), b"\x1b[?1u");
}

/// Asserts that a hard reset clears the flags of both screens.
///
/// Case: the user runs `reset` after a crashed program left flags pushed on either screen.
#[test]
fn a_hard_reset_clears_both_screens() {
    assert_eq!(
        flags_after(b"\x1b[>1u\x1b[?1049h\x1b[>1u\x1bc"),
        b"\x1b[?0u"
    );
    assert_eq!(
        flags_after(b"\x1b[>1u\x1b[?1049h\x1b[>1u\x1bc\x1b[?1049h"),
        b"\x1b[?0u"
    );
}

/// Asserts that the sequences of the `reset` command leave no flag in force.
///
/// Case: the user runs `reset` after a crashed program left flags pushed.
#[test]
fn the_reset_command_restores_the_legacy_keys() {
    assert_eq!(
        flags_after(b"\x1b[>1u\x1bc\x1b]104\x07\x1b[!p\x1b[?3;4l\x1b[4l\x1b>"),
        b"\x1b[?0u"
    );
}

/// Asserts that a soft reset clears the flags of both screens.
///
/// Case: the user runs `tput init` after a crashed program left flags pushed on either screen.
#[test]
fn a_soft_reset_clears_both_screens() {
    assert_eq!(
        flags_after(b"\x1b[>1u\x1b[?1049h\x1b[>1u\x1b[!p"),
        b"\x1b[?0u"
    );
    assert_eq!(
        flags_after(b"\x1b[>1u\x1b[?1049h\x1b[>1u\x1b[!p\x1b[?1049l"),
        b"\x1b[?0u"
    );
}

/// Asserts that a soft reset on the alternate screen clears the flags of both screens but
/// changes nothing else on the main screen.
///
/// Case: a program on the alternate screen is reset with `tput init` while the main screen's
/// shell has a red pen and flags pushed.
#[test]
fn a_soft_reset_on_the_alternate_screen_touches_only_the_main_screens_flags() {
    let (device, output) = interpret_fully(b"\x1b[31m\x1b[>1u\x1b[?47h\x1b[!p\x1b[?47lx\x1b[?u");
    assert_eq!(output.replies, b"\x1b[?0u");
    assert_eq!(cell_at(&device, 0, 0).fg, Color::Indexed(1));
}

/// Asserts that the flag reply precedes the attributes reply when both are requested in order.
///
/// Case: an application probes the keyboard flags and then sends a primary device attributes
/// query as a sentinel.
#[test]
fn the_flag_reply_precedes_the_attributes_reply() {
    assert_eq!(replies_of(b"\x1b[?u\x1b[c"), b"\x1b[?0u\x1b[?6c");
}

/// Asserts that push, pop and set requests raise no reply, signal or damage.
///
/// Case: an application changes its keyboard flags while it draws nothing.
#[test]
fn keyboard_requests_raise_nothing_but_the_reply() {
    for chunk in [&b"\x1b[>1u"[..], b"\x1b[<u", b"\x1b[=1u"] {
        let (_, output) = interpret_fully(chunk);
        assert!(output.replies.is_empty());
        assert!(output.signals.is_empty());
        assert!(!output.damaged);
    }
}
