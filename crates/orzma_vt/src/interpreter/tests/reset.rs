//! Tests for the hard terminal reset.

use super::*;

/// Asserts that `ESC c` blanks every visible row.
///
/// Case: a program dies mid-redraw and leaves the screen unusable,
/// so the user runs `reset` to take the terminal back.
#[test]
fn the_seven_bit_reset_blanks_every_visible_row() {
    let device = interpret(b"ab\r\nc\x1bc");
    for line in 0..3 {
        let row = device.active_screen().viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| *cell == Cell::default()));
    }
}

/// Asserts that the repaint `ESC c` calls for reaches the chunk
/// liveness rather than being dropped by the handler.
///
/// Case: the user runs `reset` on a screen a previous command filled,
/// and the owner must open its coalesce window for the frame that
/// repaints it.
#[test]
fn the_seven_bit_reset_marks_its_own_chunk_damaged() {
    assert!(liveness_after(b"a", b"\x1bc"));
}

/// Asserts that `ESC c` names the placements it strands in the
/// chunk's own signals.
///
/// Case: a companion app mounted a webview beside a prompt and the
/// user runs `reset`, so the host must despawn it.
#[test]
fn the_seven_bit_reset_names_the_placements_it_strands() {
    let mut session = Session::new();
    session.feed(b"ab");
    let id = InstanceId(1);
    session.mount(id);
    let output = session.feed(b"\x1bc");
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewEvicted {
            placements: vec![id]
        }]
    );
}

/// Asserts that `ESC c` on a blank screen, which stages no row
/// damage, still marks the chunk damaged when it strands a placement.
///
/// Case: a companion app mounted a webview at a fresh prompt, nothing
/// else was printed, and the user runs `reset`.
#[test]
fn a_reset_that_only_strands_a_placement_marks_the_chunk_damaged() {
    let mut session = Session::new();
    session.mount(InstanceId(1));
    session.frame();
    let output = session.feed(b"\x1bc");
    assert!(output.damaged);
}

/// Asserts that the chunk after a reset eviction raises nothing, so a
/// placement is named once.
///
/// Case: the shell prints its prompt on the frame after `reset`
/// despawned a webview.
#[test]
fn the_chunk_after_a_reset_eviction_raises_nothing() {
    let mut session = Session::new();
    session.mount(InstanceId(1));
    session.feed(b"\x1bc");
    let output = session.feed(b"a");
    assert!(output.signals.is_empty());
}
