//! Tests for `resize`: both seams take the size, the PTY writer stays
//! untouched, and a failed PTY resize leaves the VT alone.

use super::*;

/// A terminal whose PTY refuses every resize, wired without the
/// initial sizing pass so `vt.resizes` starts empty.
fn failing_term() -> OrzmaTty<FakeVt> {
    let pty = Pty::with_master(Box::new(FailingMaster), Box::new(CaptureSink::default()));
    OrzmaTty::wired(FakeVt::new(grid(80, 24)), pty)
}

fn sizes(term: &OrzmaTty<FakeVt>) -> ((u16, u16), (u16, u16)) {
    let pty = term.pty_size();
    let grid = term.vt.grid_size();
    ((pty.cols, pty.rows), (grid.cols, grid.rows))
}

/// Asserts that a detached terminal's resize round-trips through the
/// fake master and that pumping it never reports a child exit.
///
/// Case: a fixture builds a detached terminal, resizes it to the test
/// window, and pumps it for a few frames.
#[test]
fn detached_resizes_through_the_fake_master_and_never_exits() {
    let sink = CaptureSink::default();
    let mut term = OrzmaTty::detached(FakeVt::new(grid(80, 24)), grid(80, 24), Box::new(sink))
        .expect("OrzmaTty::detached");

    term.resize(grid(120, 40), CellPixels::default())
        .expect("resize");
    let size = term.pty_size();
    assert_eq!((size.cols, size.rows), (120, 40));

    for _ in 0..3 {
        assert_eq!(child_exits(&term.pump()), vec![]);
    }
}

/// Asserts that a resize reaches both seams: the PTY size read back
/// from the kernel and the `GridSize` handed to the VT.
///
/// Case: the user drags the window to a new size.
#[test]
fn resize_applies_the_size_to_both_seams() {
    let (mut term, _sink) = detached_term();
    term.resize(grid(120, 40), CellPixels::default())
        .expect("resize");
    assert_eq!(sizes(&term), ((120, 40), (120, 40)));
    assert_eq!(
        term.vt.resizes.last(),
        Some(&GridSize {
            cols: 120,
            rows: 40
        })
    );
}

/// Asserts that a resize never writes through the PTY writer, not
/// even an XTWINOPS report.
///
/// Case: the user resizes the window while a program is reading
/// stdin.
#[test]
fn resize_does_not_write_through_the_pty_writer() {
    let (mut term, sink) = detached_term();
    term.resize(grid(120, 40), CellPixels::default())
        .expect("resize");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
}

/// Asserts that a successful resize arms the coalescer.
///
/// Case: the user resizes the window at an idle shell prompt, where
/// the new grid geometry is the only thing that changes.
#[test]
fn resize_arms_the_coalescer() {
    let (mut term, _sink) = detached_term();
    term.resize(grid(120, 40), CellPixels::default())
        .expect("resize");
    assert!(term.coalescer.is_armed());
}

/// Asserts that a resize to the size the terminal already has arms
/// nothing.
///
/// Case: the host recomputes cells after a pixel-only window change
/// and re-applies the grid size the VT already holds.
#[test]
fn a_same_size_resize_does_not_arm_the_coalescer() {
    let (mut term, _sink) = detached_term();
    term.resize(grid(80, 24), CellPixels::default())
        .expect("same-size resize must be Ok");
    assert!(!term.coalescer.is_armed());
}

/// Asserts that a same-size resize leaves an already-open emit
/// window's deadline untouched.
///
/// Case: a burst of window events re-applies the current grid size
/// while an earlier repaint is still pending.
#[test]
fn a_same_size_resize_does_not_extend_the_deadline() {
    let (mut term, _sink) = detached_term();
    term.resize(grid(120, 40), CellPixels::default())
        .expect("resize");
    let deadline = term.coalescer.next_deadline();
    assert!(deadline.is_some(), "precondition: a real resize arms");
    term.resize(grid(120, 40), CellPixels::default())
        .expect("same-size resize must be Ok");
    assert_eq!(term.coalescer.next_deadline(), deadline);
}

/// Asserts that back-to-back resizes settle on the last requested
/// size on both seams.
///
/// Case: a live window drag fires a burst of requests.
#[test]
fn sequential_resizes_settle_on_the_last_size() {
    let (mut term, _sink) = detached_term();
    term.resize(grid(120, 40), CellPixels::default())
        .expect("resize");
    term.resize(grid(90, 30), CellPixels::default())
        .expect("resize");
    assert_eq!(sizes(&term), ((90, 30), (90, 30)));
}

/// Asserts PTY-first ordering via failure atomicity: when the PTY
/// ioctl fails, the call returns `PtyResize` and the VT and
/// coalescer are untouched.
///
/// Case: the kernel refuses the winsize ioctl while the user resizes
/// the window.
#[test]
fn a_failing_pty_resize_leaves_the_vt_untouched() {
    let mut term = failing_term();
    let result = term.resize(grid(120, 40), CellPixels::default());
    assert!(
        matches!(result, Err(OrzmaTtyError::PtyResize(_))),
        "expected PtyResize, got {result:?}"
    );
    assert!(term.vt.resizes.is_empty());
    assert!(!term.coalescer.is_armed());
}
