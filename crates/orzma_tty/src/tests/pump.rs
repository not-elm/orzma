//! Tests for `pump`: chunk draining, child-exit detection and its
//! single report, and the order of the final output and signals.

use super::*;

/// Asserts that one pump interprets at most `MAX_CHUNKS_PER_PUMP`
/// chunks and reports the remainder as pending.
///
/// Case: a `cat` of a large file floods the PTY faster than one pump
/// can drain it while other panes wait their turn.
#[test]
fn a_pump_drains_at_most_the_chunk_budget() {
    let (mut tty, chunk_tx, _exit_tx) = channelled_term();
    for _ in 0..(OrzmaTty::<FakeVt>::MAX_CHUNKS_PER_PUMP + 6) {
        chunk_tx.send(b"x".to_vec()).unwrap();
    }
    let first = tty.pump();
    assert!(first.more_pending);
    assert_eq!(
        tty.vt.interpreted.len(),
        OrzmaTty::<FakeVt>::MAX_CHUNKS_PER_PUMP
    );
    let second = tty.pump();
    assert!(!second.more_pending);
    assert_eq!(
        tty.vt.interpreted.len(),
        OrzmaTty::<FakeVt>::MAX_CHUNKS_PER_PUMP + 6
    );
}

/// Asserts that `pending_chunk_count` reports the chunks queued and
/// unread, and drops to zero once a pump interprets them.
///
/// Case: the backend samples a pane's chunk queue right after its
/// `Select` woke and before it pumps the pane.
#[test]
fn pending_chunk_count_reports_the_unread_queue() {
    let (mut tty, chunk_tx, _exit_tx) = channelled_term();
    assert_eq!(tty.pending_chunk_count(), 0);
    for _ in 0..3 {
        chunk_tx.send(b"x".to_vec()).unwrap();
    }
    assert_eq!(tty.pending_chunk_count(), 3);
    tty.pump();
    assert_eq!(tty.pending_chunk_count(), 0);
}

/// Asserts that `ChildExit` is withheld while chunks remain and is
/// then reported once, last, on the pump that drains the rest.
///
/// Case: the shell prints a long farewell and exits; the reader
/// thread queues every chunk before the exit status.
#[test]
fn child_exit_waits_for_the_remaining_chunks_and_is_reported_once() {
    let (mut tty, chunk_tx, exit_tx) = channelled_term();
    for _ in 0..(OrzmaTty::<FakeVt>::MAX_CHUNKS_PER_PUMP + 1) {
        chunk_tx.send(b"bye".to_vec()).unwrap();
    }
    exit_tx.send(Some(0)).unwrap();
    drop(chunk_tx);
    drop(exit_tx);

    let first = tty.pump();
    assert!(first.more_pending);
    assert!(
        !first
            .signals
            .iter()
            .any(|s| matches!(s, TtySignal::ChildExit { .. }))
    );
    assert!(
        tty.readiness().exit.is_none(),
        "exit is latched after the first pump"
    );

    let second = tty.pump();
    assert!(!second.more_pending);
    assert_eq!(
        second.signals.last(),
        Some(&TtySignal::ChildExit { code: Some(0) })
    );

    let third = tty.pump();
    assert!(
        !third
            .signals
            .iter()
            .any(|s| matches!(s, TtySignal::ChildExit { .. }))
    );
}

/// Asserts that a reader thread that vanished without sending an exit
/// status still yields exactly one `ChildExit { code: None }`.
///
/// Case: the reader thread panics, dropping both senders before the
/// exit status was sent.
#[test]
fn both_streams_disconnected_without_a_status_synthesize_one_child_exit() {
    let (mut tty, chunk_tx, exit_tx) = channelled_term();
    drop(chunk_tx);
    drop(exit_tx);
    let first = tty.pump();
    assert_eq!(first.signals, vec![TtySignal::ChildExit { code: None }]);
    assert!(!first.more_pending);
    assert!(tty.readiness().exit.is_none());
    let second = tty.pump();
    assert!(second.signals.is_empty());
}

/// Asserts that a pending child-exit report surfaces in `pump`'s
/// signals as `ChildExit` carrying the reported code.
///
/// Case: the child exits with a nonzero code while the terminal is
/// otherwise idle, and the host pumps on the next frame.
#[test]
fn pump_surfaces_child_exit_with_the_reported_code() {
    let (mut term, _chunk_tx, exit_tx) = channelled_term();
    exit_tx.send(Some(3)).expect("send exit");
    assert_eq!(child_exits(&term.pump().signals), vec![Some(3)]);
}

/// Asserts that a failed `wait` surfaces as `ChildExit` with
/// `code: None` rather than being dropped.
///
/// Case: the reader thread's `wait` on the exited child fails, so
/// no exit code exists to report.
#[test]
fn pump_surfaces_a_wait_failure_as_code_none() {
    let (mut term, _chunk_tx, exit_tx) = channelled_term();
    exit_tx.send(None).expect("send exit");
    assert_eq!(child_exits(&term.pump().signals), vec![None]);
}

/// Asserts that `ChildExit` appears in exactly one `pump` result
/// and never again on later calls.
///
/// Case: the shell exits while the host keeps pumping every frame.
#[test]
fn child_exit_is_emitted_exactly_once_across_pumps() {
    let (mut term, _chunk_tx, exit_tx) = channelled_term();
    exit_tx.send(Some(0)).expect("send exit");
    assert_eq!(child_exits(&term.pump().signals), vec![Some(0)]);
    for _ in 0..3 {
        assert_eq!(child_exits(&term.pump().signals), vec![]);
    }
}

/// Asserts that a detached terminal — one with no reader thread and
/// so no child to report on — never emits `ChildExit`.
///
/// Case: a detached test terminal is pumped every frame like a
/// live one.
#[test]
fn pump_on_a_detached_terminal_never_emits_child_exit() {
    let (mut term, _sink) = detached_term();
    for _ in 0..3 {
        assert_eq!(child_exits(&term.pump().signals), vec![]);
    }
}

/// Asserts that a `pump` which reports `ChildExit` has already
/// interpreted every pending output chunk.
///
/// Case: the child runs `echo bye`, so the reader thread delivers the
/// final output chunk and then the exit report, and the host pumps
/// once after both arrived.
#[test]
fn the_final_output_is_interpreted_when_the_exit_is_reported() {
    let (mut term, chunk_tx, exit_tx) = channelled_term();
    chunk_tx.send(b"bye".to_vec()).expect("send chunk");
    exit_tx.send(Some(0)).expect("send exit");
    assert_eq!(child_exits(&term.pump().signals), vec![Some(0)]);
    assert!(term.vt.interpreted.contains(&b"bye".to_vec()));
}

/// Asserts that VT signals from interpreted chunks surface as
/// `TtySignal::Vt`, ahead of a `ChildExit` in the same batch.
///
/// Case: the shell rings the bell in its final output and exits.
#[test]
fn vt_signals_are_forwarded_before_child_exit() {
    let (mut term, chunk_tx, exit_tx) = channelled_term();
    term.vt.updates.push_back(InterpretOutput {
        damaged: true,
        signals: vec![VtSignal::Bell],
        replies: Vec::new(),
        consumed: 1,
        synchronized_update_closed: false,
    });
    chunk_tx.send(b"\x07".to_vec()).expect("send chunk");
    exit_tx.send(Some(0)).expect("send exit");
    let signals = term.pump().signals;
    assert_eq!(
        signals,
        vec![
            TtySignal::Vt(VtSignal::Bell),
            TtySignal::ChildExit { code: Some(0) }
        ]
    );
}

/// Asserts that reply bytes from interpreted chunks are written
/// back to the PTY by the next pump.
///
/// Case: an application sends a DSR cursor-position query and
/// blocks until the report arrives.
#[test]
fn replies_are_written_back_to_the_pty() {
    let (chunk_tx, chunk_rx) = unbounded();
    let (_exit_tx, exit_rx) = unbounded();
    let sink = CaptureSink::default();
    let pty = Pty::with_master_and_channels(
        Box::new(FailingMaster),
        Box::new(sink.clone()),
        chunk_rx,
        exit_rx,
    );
    let mut term = OrzmaTty::wired(FakeVt::new(grid(80, 24)), pty);
    term.vt.updates.push_back(InterpretOutput {
        damaged: true,
        signals: Vec::new(),
        replies: b"\x1b[1;1R".to_vec(),
        consumed: 4,
        synchronized_update_closed: false,
    });
    chunk_tx.send(b"\x1b[6n".to_vec()).expect("send chunk");
    term.pump();
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[1;1R");
}
