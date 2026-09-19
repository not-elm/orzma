//! Tests for the frame coalescer seam: the bootstrap frame, deadlines,
//! `flush_now`, and what arms a repaint window.

use super::*;

/// Asserts that `next_deadline` is due immediately while the
/// bootstrap frame is owed and `None` once it settled with nothing
/// armed.
///
/// Case: the backend computes its select timeout for a freshly
/// spawned, silent pane.
#[test]
fn next_deadline_is_now_until_the_bootstrap_frame_settles() {
    let (mut tty, _chunk_tx, _exit_tx) = channelled_term();
    let now = Instant::now();
    assert!(tty.next_deadline(now).is_some());
    tty.vt.frames.push_back(a_frame());
    tty.pump();
    assert!(tty.next_deadline(now).is_none());
}

/// Asserts that the deadline reported for an owed bootstrap frame is the
/// caller's own `now`, so a caller comparing the two finds it due.
///
/// Case: the backend samples one clock read for a whole loop turn, then
/// asks a freshly spawned pane whether it wants a pump.
#[test]
fn an_owed_bootstrap_frame_is_due_at_the_callers_now() {
    let (tty, _sink) = detached_term();
    let now = Instant::now();
    assert_eq!(tty.next_deadline(now), Some(now));
}

/// Asserts that `flush_now` returns the pending signals and an
/// immediate frame without reading the PTY.
///
/// Case: the backend resizes a pane and sends its repaint in the same
/// batch as the new layout.
#[test]
fn flush_now_returns_pending_signals_and_an_immediate_frame() {
    let (mut tty, chunk_tx, _exit_tx) = channelled_term();
    tty.vt.evictions.push_back(vec![InstanceId(7)]);
    tty.resize(grid(40, 12), CellPixels::default()).unwrap();
    tty.vt.frames.push_back(a_frame());
    chunk_tx.send(b"unread".to_vec()).unwrap();

    let out = tty.flush_now();
    assert!(out.frames().count() == 1);
    assert_eq!(
        out.signals().cloned().collect::<Vec<_>>(),
        vec![TtySignal::Vt(VtSignal::WebviewEvicted {
            placements: vec![InstanceId(7)]
        })]
    );
    assert!(!out.more_pending);
    assert!(
        tty.vt.interpreted.is_empty(),
        "flush_now must not read the PTY"
    );
}

/// Asserts that the first pump returns the bootstrap frame with no
/// PTY output having arrived, and that a second pump right after
/// returns none.
///
/// Case: a freshly spawned terminal is pumped before the shell
/// prints its first byte, such as a silent shell sitting at an
/// empty prompt.
#[test]
fn the_first_pump_returns_the_bootstrap_frame_even_with_no_output() {
    let (mut tty, _sink) = detached_term();
    tty.vt.frames.push_back(a_frame());
    let first = tty.pump();
    assert!(first.frames().count() == 1);
    let second = tty.pump();
    assert!(second.frames().next().is_none());
}

/// Asserts that a bootstrap pump whose VT has no frame ready yet
/// keeps the bootstrap debt owed, so the very next pump still asks
/// for it instead of skipping the initial snapshot.
///
/// Case: the coalescer's bootstrap flag comes due before the VT has
/// assembled anything to hand back.
#[test]
fn a_bootstrap_pump_with_no_frame_ready_keeps_the_debt_for_the_next_pump() {
    let (mut tty, _sink) = detached_term();
    let first = tty.pump();
    assert!(first.frames().next().is_none());
    assert!(tty.coalescer.needs_bootstrap());

    tty.vt.frames.push_back(a_frame());
    let second = tty.pump();
    assert!(second.frames().count() == 1);
}

/// Asserts that a chunk arriving before the first pump — which both
/// arms the coalescer and owes the bootstrap emit — still produces
/// exactly one frame, not two.
///
/// Case: the shell prints its prompt before the host's first pump
/// call after spawn.
#[test]
fn a_pre_pump_chunk_does_not_double_emit_the_bootstrap_frame() {
    let (mut tty, _sink) = detached_term();
    tty.vt.frames.push_back(a_frame());
    tty.vt.frames.push_back(a_frame());
    tty.feed_bytes(b"$ ");
    let first = tty.pump();
    assert!(first.frames().count() == 1);
    let second = tty.pump();
    assert!(second.frames().next().is_none());
}

/// Asserts that the placements a resize strands reach the next
/// pump's signals, without any PTY output to carry them.
///
/// Case: the user drags the window shorter, dropping the anchor
/// row of a mounted webview out of scrollback, and types nothing
/// afterwards.
#[test]
fn a_resize_eviction_reaches_the_next_pump() {
    let (mut tty, _sink) = detached_term();
    tty.vt.evictions.push_back(vec![InstanceId(7)]);
    tty.resize(grid(100, 30), CellPixels::default())
        .expect("resize");
    assert_eq!(
        tty.pump().signals().cloned().collect::<Vec<_>>(),
        vec![TtySignal::Vt(VtSignal::WebviewEvicted {
            placements: vec![InstanceId(7)]
        })]
    );
}

/// Asserts that a pump with nothing evicted raises no signal and
/// arms nothing.
///
/// Case: the host pumps a quiet terminal that has no webviews
/// mounted.
#[test]
fn a_pump_with_nothing_evicted_raises_nothing() {
    let (mut tty, _sink) = detached_term();
    let output = tty.pump();
    assert!(output.signals().next().is_none());
    assert!(!tty.coalescer.is_armed());
}

/// Asserts that a chunk which stages damage arms the coalesce
/// window.
///
/// Case: the shell echoes a typed character at an idle prompt.
#[test]
fn a_chunk_that_stages_damage_arms_the_window() {
    let (mut term, _sink) = detached_term();
    term.feed_bytes(b"a");
    assert!(term.coalescer.is_armed());
}

/// Asserts that a chunk which stages no damage leaves the coalesce
/// window closed.
///
/// Case: a program queries the cursor position, so the VT answers
/// with reply bytes and touches no cell.
#[test]
fn a_chunk_that_stages_no_damage_does_not_arm_the_window() {
    let (mut term, _sink) = detached_term();
    term.vt.updates.push_back(InterpretOutput {
        damaged: false,
        signals: Vec::new(),
        replies: b"\x1b[1;1R".to_vec(),
        consumed: 4,
        synchronized_update_closed: false,
    });
    term.feed_bytes(b"\x1b[6n");
    assert!(!term.coalescer.is_armed());
}
