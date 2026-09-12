//! What one interpreted chunk reports back: its liveness, its
//! signals, and the replies it accumulated.

use super::*;

/// Asserts that staging row damage marks the chunk damaged.
///
/// Case: a shell echoes the character the user just typed.
#[test]
fn staged_row_damage_marks_the_chunk_damaged() {
    assert!(damage_of(b"a"));
}

/// Asserts that both bells reach the signals and leave the chunk
/// undamaged.
///
/// Case: a shell rings the bell twice for an ambiguous completion,
/// printing nothing.
#[test]
fn a_bell_reaches_the_signals() {
    let (_device, output) = interpret_fully(b"\x07\x07");
    assert_eq!(output.signals, vec![VtSignal::Bell, VtSignal::Bell]);
    assert!(!output.damaged);
}

/// Asserts that a reply leaves the chunk undamaged.
///
/// Case: an application probes the terminal while the screen sits
/// untouched at a prompt.
#[test]
fn a_reply_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b[c"));
}

/// Asserts that two requests in one chunk both reach the replies,
/// concatenated in the order they arrived.
///
/// Case: an application flushes its whole capability probe in a
/// single write.
#[test]
fn replies_accumulate_within_one_chunk() {
    assert_eq!(replies_of(b"\x1b[c\x1b[c"), b"\x1b[?6c\x1b[?6c");
}
