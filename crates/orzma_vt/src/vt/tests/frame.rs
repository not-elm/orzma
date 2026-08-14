//! Frame-emission tests: Snapshot/Delta classification, staged-damage
//! consumption, sequence numbering, and payload contents.

use super::*;
use crate::schema::{FrameDelta, FrameSnapshot, Rgb, ViewportLine};

fn snapshot(vt: &mut OrzmaVt<AlacrittyVtBackend>) -> FrameSnapshot {
    match vt.frame() {
        Some(Frame::Snapshot(snapshot)) => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn delta(vt: &mut OrzmaVt<AlacrittyVtBackend>) -> FrameDelta {
    match vt.frame() {
        Some(Frame::Delta(delta)) => delta,
        other => panic!("expected a delta, got {other:?}"),
    }
}

fn dirty_lines(delta: &FrameDelta) -> Vec<ViewportLine> {
    delta.dirty_rows.iter().map(|row| row.line).collect()
}

/// Asserts that a fresh VT emits a bootstrap snapshot describing the
/// full viewport.
///
/// Case: the renderer draws its very first frame for a terminal whose
/// shell has just printed its prompt.
#[test]
fn a_fresh_vt_emits_a_bootstrap_snapshot_of_the_full_viewport() {
    let mut vt = OrzmaVt::<AlacrittyVtBackend>::new(80, 24);
    vt.interpret(b"abc");
    let snap = snapshot(&mut vt);
    assert_eq!(snap.seq, 0);
    assert_eq!(snap.size, GridSize { cols: 80, rows: 24 });
    assert_eq!(snap.rows.len(), 24);
    assert!(row_text(&snap.rows[0]).starts_with("abc"));
    for row in &snap.rows {
        assert_eq!(
            row.runs.iter().map(|run| u32::from(run.cols)).sum::<u32>(),
            80
        );
    }
    assert_eq!(snap.display_offset, DisplayOffset(0));
    assert_eq!(snap.history_size, 0);
    assert_eq!(snap.history_base, 0);
    assert_eq!(snap.vi_cursor, None);
    assert_eq!(snap.selection, None);
    assert!(snap.hyperlinks.is_empty());
}

/// Asserts that emitting a frame consumes the staged damage.
///
/// Case: the emit loop runs twice with no terminal activity in
/// between; the second pass has nothing to hand the renderer.
#[test]
fn frame_consumes_the_staged_damage() {
    let mut vt = OrzmaVt::<AlacrittyVtBackend>::new(80, 24);
    assert!(vt.frame().is_some());
    assert!(vt.frame().is_none());
}

/// Asserts that staged row damage emits a delta carrying the dirty
/// rows' contents.
///
/// Case: the shell echoes a few typed characters and only the prompt
/// row needs repainting.
#[test]
fn row_damage_emits_a_delta_with_the_dirty_row_contents() {
    let mut vt = clean_vt();
    vt.interpret(b"abc");
    let delta = delta(&mut vt);
    assert_eq!(delta.dirty_rows.len(), 1);
    assert_eq!(delta.dirty_rows[0].line, ViewportLine(0));
    assert!(row_text(&delta.dirty_rows[0].contents).starts_with("abc"));
    assert!(delta.hyperlinks.is_empty());
}

/// Asserts that staged full damage emits a snapshot.
///
/// Case: a mouse press anchors a selection, whose repaint the
/// backend's damage tracking cannot scope to rows.
#[test]
fn full_damage_emits_a_snapshot() {
    let mut vt = clean_vt();
    start_simple(&mut vt, 0, 0);
    assert!(matches!(vt.frame(), Some(Frame::Snapshot(_))));
}

/// Asserts that damage merged across chunks lists every dirty row in
/// one delta.
///
/// Case: two PTY chunks land between emits — echoed input on the top
/// row, then a status-line update rows below it.
#[test]
fn merged_partial_damage_lists_every_dirty_row() {
    let mut vt = clean_vt();
    vt.interpret(b"a");
    vt.interpret(b"\x1b[6;1Hb");
    let delta = delta(&mut vt);
    let lines = dirty_lines(&delta);
    assert!(lines.contains(&ViewportLine(0)), "got {lines:?}");
    assert!(lines.contains(&ViewportLine(5)), "got {lines:?}");
}

/// Asserts that full damage staged after row damage decides the frame
/// kind.
///
/// Case: output repaints a row, then a selection lands before the
/// emit; the whole viewport must repaint, not just the row.
#[test]
fn full_absorbs_partial_for_the_frame_kind() {
    let mut vt = clean_vt();
    vt.interpret(b"abc");
    start_simple(&mut vt, 0, 0);
    assert!(matches!(vt.frame(), Some(Frame::Snapshot(_))));
}

/// Asserts that a delta reports the current cursor and overlay state.
///
/// Case: the user holds a selection while typing; each echoed
/// character's delta must carry the moved cursor and the still-active
/// selection.
#[test]
fn a_delta_reports_the_current_overlay_state() {
    let mut vt = clean_vt();
    start_simple(&mut vt, 0, 0);
    vt.frame();
    vt.interpret(b"x");
    let delta = delta(&mut vt);
    assert_eq!(delta.cursor.point.column, GridColumn(1));
    assert!(delta.selection.is_some());
    assert_eq!(delta.display_offset, DisplayOffset(0));
    assert_eq!(delta.vi_cursor, None);
}

/// Asserts that the sequence number advances only when a frame is
/// actually emitted.
///
/// Case: the emit loop polls between activity bursts; idle polls must
/// not open gaps in the sequence the renderer tracks.
#[test]
fn seq_advances_only_when_a_frame_is_emitted() {
    let mut vt = OrzmaVt::<AlacrittyVtBackend>::new(80, 24);
    assert_eq!(snapshot(&mut vt).seq, 0);
    assert!(vt.frame().is_none());
    vt.interpret(b"a");
    assert_eq!(delta(&mut vt).seq, 1);
    vt.interpret(b"b");
    assert_eq!(delta(&mut vt).seq, 2);
}

/// Asserts that the sequence wraps from `u32::MAX` to zero.
///
/// Case: a long-lived session eventually exhausts the counter; the
/// renderer's not-equal comparison keeps working across the wrap, and
/// the emitter must not panic on overflow.
#[test]
fn seq_wraps_at_u32_max() {
    let mut vt = clean_vt();
    vt.next_frame_seq = u32::MAX;
    vt.interpret(b"a");
    assert_eq!(delta(&mut vt).seq, u32::MAX);
    vt.interpret(b"b");
    assert_eq!(delta(&mut vt).seq, 0);
}

/// Asserts that a scrolled snapshot renders the viewport the user
/// scrolled to.
///
/// Case: the user wheels back into history and the whole window
/// repaints with the older lines.
#[test]
fn a_scrolled_snapshot_renders_the_scrolled_viewport() {
    let mut vt = vt_with_history(17);
    vt.scroll(Scroll::Delta(3));
    let snap = snapshot(&mut vt);
    assert_eq!(snap.display_offset, DisplayOffset(3));
    assert_eq!(snap.history_size, 17);
    assert!(
        row_text(&snap.rows[0]).starts_with("l14"),
        "got {:?}",
        row_text(&snap.rows[0])
    );
}

/// Asserts that a resize snapshot reports the new dimensions and row
/// count.
///
/// Case: the user drags the window larger and the next frame redraws
/// the reflowed grid at its new size.
#[test]
fn a_resize_snapshot_reports_the_new_size() {
    let mut vt = clean_vt();
    assert!(vt.resize(100, 30));
    let snap = snapshot(&mut vt);
    assert_eq!(
        snap.size,
        GridSize {
            cols: 100,
            rows: 30
        }
    );
    assert_eq!(snap.rows.len(), 30);
}

/// Asserts that a palette override reaches the next snapshot's
/// resolution table.
///
/// Case: a theming tool recolors palette slot 1 with OSC 4, and the
/// full repaint that follows resolves indexed cells against the new
/// value.
#[test]
fn a_palette_override_reaches_the_next_snapshot() {
    let mut vt = clean_vt();
    vt.interpret(b"\x1b]4;1;rgb:ff/00/00\x07");
    let snap = snapshot(&mut vt);
    assert_eq!(snap.palette.indexed[1], Rgb { r: 255, g: 0, b: 0 });
}

/// Asserts that an empty delta is still emitted with fresh metadata
/// and consumed like any other frame.
///
/// Case: the user reads deep scrollback while the shell keeps
/// printing; nothing visible changes, but the cursor metadata the
/// frame carries has.
#[test]
fn an_empty_delta_still_reports_fresh_metadata() {
    let mut vt = vt_with_history(30);
    vt.scroll(Scroll::Top);
    vt.frame();
    vt.interpret(b"x");
    let delta = delta(&mut vt);
    assert!(delta.dirty_rows.is_empty());
    assert_eq!(delta.cursor.point.column, GridColumn(1));
    assert_eq!(delta.display_offset, DisplayOffset(30));
    assert!(vt.frame().is_none());
}
