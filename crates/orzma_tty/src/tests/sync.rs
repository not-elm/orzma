//! Tests for synchronized updates: the emit suppression, its timeout,
//! the frame taken when an update closes, and the contract defences.

use super::*;

/// A terminal whose bootstrap frame is already painted.
fn painted_term() -> (OrzmaTty<FakeVt>, CaptureSink) {
    let (mut tty, sink) = detached_term();
    tty.vt.frames.push_back(a_frame());
    assert_eq!(tty.pump().frames().count(), 1);
    (tty, sink)
}

/// Opens a synchronized update with one damaging chunk.
fn open_update(tty: &mut OrzmaTty<FakeVt>) {
    tty.vt.sync_script.push_back(SynchronizedOutput::Active);
    tty.feed_bytes(b"x")
        .expect("the fake VT honors the interpret contract");
}

/// Closes the open synchronized update with one chunk that has a frame
/// ready.
fn close_update(tty: &mut OrzmaTty<FakeVt>) {
    tty.vt.sync_script.push_back(SynchronizedOutput::Inactive);
    tty.vt.updates.push_back(update(1, true));
    tty.vt.frames.push_back(a_frame());
    tty.feed_bytes(b"z")
        .expect("the fake VT honors the interpret contract");
}

/// Asserts that a pump emits no frame while a synchronized update is
/// open, even once the coalesce window is due.
///
/// Case: nvim is halfway through redrawing its screen inside a
/// synchronized update when the coalesce window elapses.
#[test]
fn an_open_update_holds_back_the_due_frame() {
    let (mut tty, _sink) = painted_term();
    open_update(&mut tty);
    tty.sync_deadline = Some(Instant::now() + Duration::from_secs(60));
    tty.vt.frames.push_back(a_frame());
    thread::sleep(Duration::from_millis(5));
    assert!(tty.pump().frames().next().is_none());
    assert_eq!(tty.vt.frames.len(), 1);
}

/// Asserts that a pump emits no bootstrap frame while a synchronized
/// update is open.
///
/// Case: a program started in a fresh pane opens a synchronized
/// update with its very first write.
#[test]
fn an_open_update_holds_back_the_bootstrap_frame() {
    let (mut tty, _sink) = detached_term();
    open_update(&mut tty);
    tty.sync_deadline = Some(Instant::now() + Duration::from_secs(60));
    tty.vt.frames.push_back(a_frame());
    assert!(tty.pump().frames().next().is_none());
    assert!(tty.coalescer.needs_bootstrap());
}

/// Asserts that an open update makes its own deadline the only one
/// the owner is asked to wake for.
///
/// Case: the backend computes how long it may sleep while a pane has
/// an update open and damage already staged.
#[test]
fn an_open_update_reports_only_its_own_deadline() {
    let (mut tty, _sink) = detached_term();
    open_update(&mut tty);
    assert!(tty.sync_deadline.is_some());
    assert_eq!(tty.next_deadline(Instant::now()), tty.sync_deadline);
}

/// Asserts that the deadline is anchored at the first open and that a
/// repeated open does not move it.
///
/// Case: an application sends `DECSET 2026` again while its update is
/// still open.
#[test]
fn a_repeated_open_does_not_move_the_deadline() {
    let (mut tty, _sink) = painted_term();
    open_update(&mut tty);
    let deadline = tty.sync_deadline;
    open_update(&mut tty);
    assert_eq!(tty.sync_deadline, deadline);
}

/// Asserts that a pump emits again once the deadline has passed, and
/// that the update is not reopened while the VT still reports it
/// open.
///
/// Case: an application opens a synchronized update and dies before
/// it closes it.
#[test]
fn a_timed_out_update_stops_holding_frames_back() {
    let (mut tty, _sink) = painted_term();
    open_update(&mut tty);
    let expired = Instant::now();
    tty.sync_deadline = Some(expired);
    tty.vt.frames.push_back(a_frame());
    thread::sleep(Duration::from_millis(5));
    assert_eq!(tty.pump().frames().count(), 1);

    tty.vt.sync_script.push_back(SynchronizedOutput::Active);
    tty.feed_bytes(b"y")
        .expect("the fake VT honors the interpret contract");
    assert_eq!(tty.sync_deadline, Some(expired));
}

/// Asserts that a close arriving after the timeout clears the deadline
/// and yields a frame at once, as a timely close does.
///
/// Case: an application stalls for a second inside a synchronized
/// update and then finishes its redraw.
#[test]
fn a_late_close_clears_the_deadline_and_yields_a_frame() {
    let (mut tty, _sink) = painted_term();
    open_update(&mut tty);
    tty.sync_deadline = Some(Instant::now());
    thread::sleep(OrzmaTty::<FakeVt>::SYNC_EMIT_INTERVAL);
    close_update(&mut tty);
    assert!(tty.sync_deadline.is_none());
    assert!(matches!(tty.pending.as_slice(), [PumpItem::Frame(_)]));
}

/// Asserts that a closed update clears the deadline and yields a frame
/// at once, ahead of the coalesce window.
///
/// Case: fzf finishes one redraw of its list.
#[test]
fn a_closed_update_yields_a_frame_at_once() {
    let (mut tty, _sink) = painted_term();
    open_update(&mut tty);
    thread::sleep(OrzmaTty::<FakeVt>::SYNC_EMIT_INTERVAL);
    close_update(&mut tty);
    assert!(tty.sync_deadline.is_none());
    assert!(matches!(tty.pending.as_slice(), [PumpItem::Frame(_)]));
}

/// Asserts that a second close inside the emit interval takes no
/// frame of its own and leaves the coalesce window armed for its
/// damage.
///
/// Case: a program toggles synchronized output around every line it
/// prints, many times within one PTY chunk.
#[test]
fn closes_inside_the_emit_interval_share_one_frame() {
    let (mut tty, _sink) = painted_term();
    thread::sleep(OrzmaTty::<FakeVt>::SYNC_EMIT_INTERVAL);
    tty.vt.updates.push_back(update(1, true));
    tty.vt.updates.push_back(update(1, true));
    tty.vt.frames.push_back(a_frame());
    tty.vt.frames.push_back(a_frame());
    tty.feed_bytes(b"ab")
        .expect("the fake VT honors the interpret contract");
    assert_eq!(tty.vt.interpreted.len(), 2);
    assert_eq!(tty.vt.frames.len(), 1);
    assert!(tty.coalescer.is_armed());
}

/// Asserts that a close with nothing to paint leaves the bootstrap
/// debt owed.
///
/// Case: a program opens and closes a synchronized update without
/// drawing anything before the pane has painted its first frame.
#[test]
fn an_empty_update_keeps_the_bootstrap_debt() {
    let (mut tty, _sink) = detached_term();
    tty.vt.updates.push_back(update(1, true));
    tty.feed_bytes(b"x")
        .expect("the fake VT honors the interpret contract");
    assert!(tty.coalescer.needs_bootstrap());
    assert!(tty.pending.is_empty());
}

/// Asserts that `flush_now` emits a frame although an update is open,
/// and leaves the update open.
///
/// Case: the user drags the window edge while nvim is inside a
/// synchronized update.
#[test]
fn flush_now_emits_through_an_open_update() {
    let (mut tty, _sink) = painted_term();
    open_update(&mut tty);
    tty.vt.frames.push_back(a_frame());
    assert_eq!(tty.flush_now().frames().count(), 1);
    assert!(tty.sync_deadline.is_some());
}

/// Asserts that a frame taken at a close sits between the signals
/// raised before it and those raised after it.
///
/// Case: a program rings the bell inside an update, closes it, and
/// sets the window title right behind it in the same PTY chunk.
#[test]
fn a_close_frame_keeps_its_place_among_the_signals() {
    let (mut tty, _sink) = painted_term();
    thread::sleep(OrzmaTty::<FakeVt>::SYNC_EMIT_INTERVAL);
    let mut first = update(1, true);
    first.signals.push(VtSignal::Bell);
    let mut second = update(1, false);
    second.signals.push(VtSignal::ResetTitle);
    tty.vt.updates.push_back(first);
    tty.vt.updates.push_back(second);
    tty.vt.frames.push_back(a_frame());
    tty.feed_bytes(b"ab")
        .expect("the fake VT honors the interpret contract");
    assert!(matches!(
        tty.pending.as_slice(),
        [
            PumpItem::Signal(TtySignal::Vt(VtSignal::Bell)),
            PumpItem::Frame(_),
            PumpItem::Signal(TtySignal::Vt(VtSignal::ResetTitle)),
        ]
    ));
}

/// Asserts that a VT which interprets none of a non-empty chunk is cut
/// off after one call, reporting [`OrzmaTtyError::VtConsumedNothing`]
/// instead of looping on the unreduced chunk.
///
/// Case: a VT implementation with a bug in its byte accounting is
/// plugged into the terminal, and a program prints a line.
#[test]
fn a_vt_that_interprets_nothing_is_cut_off() {
    let (mut tty, _sink) = painted_term();
    tty.vt.updates.push_back(update(0, false));

    let error = tty
        .feed_bytes(b"abc")
        .expect_err("a VT that interprets nothing breaks the contract");

    assert!(matches!(error, OrzmaTtyError::VtConsumedNothing { len: 3 }));
    assert_eq!(tty.vt.interpreted.len(), 1);
}

/// Asserts that a VT which claims more bytes than the chunk held is cut
/// off after one call, reporting
/// [`OrzmaTtyError::VtConsumedBeyondChunk`] instead of panicking on the
/// slice.
///
/// Case: a VT implementation with a bug in its byte accounting is
/// plugged into the terminal, and a program prints a line.
#[test]
fn a_vt_that_interprets_beyond_the_chunk_is_cut_off() {
    let (mut tty, _sink) = painted_term();
    tty.vt.updates.push_back(update(9, false));

    let error = tty
        .feed_bytes(b"abc")
        .expect_err("a VT that overruns the chunk breaks the contract");

    assert!(matches!(
        error,
        OrzmaTtyError::VtConsumedBeyondChunk {
            consumed: 9,
            len: 3
        }
    ));
    assert_eq!(tty.vt.interpreted.len(), 1);
}
