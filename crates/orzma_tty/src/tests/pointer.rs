//! Tests for `send_pointer`: reports reach the PTY in the VT's encoding,
//! and selection effects reach the VT at the display offset.

use super::*;
use crate::input::{PointerButton, PointerInput, PointerKind};
use crate::test_support::SelectionOp;

fn event(kind: PointerKind, button: Option<PointerButton>, col: u32, row: u32) -> PointerInput {
    PointerInput {
        kind,
        button,
        cell: CellCoord { col, row },
        side: CellSide::Left,
        click_count: 1,
        mods: ProtocolModifiers::default(),
    }
}

fn press(button: PointerButton, col: u32, row: u32) -> PointerInput {
    event(PointerKind::Press, Some(button), col, row)
}

fn motion(col: u32, row: u32) -> PointerInput {
    event(PointerKind::Motion, None, col, row)
}

fn release(button: PointerButton, col: u32, row: u32) -> PointerInput {
    event(PointerKind::Release, Some(button), col, row)
}

/// Asserts that a forwarded press clears the selection and writes its
/// report in the VT's SGR encoding, in one write.
///
/// Case: nvim tracks button events with SGR reports, and the user clicks
/// in its buffer.
#[test]
fn a_forwarded_press_writes_an_sgr_report() {
    let (mut term, sink) = tracking_term();
    let copied = term
        .send_pointer(press(PointerButton::Left, 5, 7))
        .expect("send_pointer");
    term.settle_writes();
    assert_eq!(copied, None);
    assert_eq!(sink.contents(), b"\x1b[<0;5;7M");
    assert_eq!(sink.writes(), 1);
    assert_eq!(term.vt.selections, vec![SelectionOp::Clear]);
}

/// Asserts that reports follow the VT's X10 encoding when SGR is off.
///
/// Case: an older TUI turns on button tracking without SGR reports, and
/// the user clicks in it.
#[test]
fn a_forwarded_press_follows_the_x10_encoding() {
    let (mut term, sink) = tracking_term();
    term.vt.modes.mouse_encoding = MouseEncoding::X10;
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("send_pointer");
    term.settle_writes();
    assert_eq!(sink.contents(), vec![0x1b, b'[', b'M', 32, 33, 33]);
}

/// Asserts that a drag in a scrolled-back pane selects at the grid points
/// its viewport cells show, writes nothing to the PTY, and leaves the
/// viewport where it is.
///
/// Case: `fzf --height` tracks the mouse on the primary screen, the user
/// has scrolled the pane three rows back into history, and drags across
/// two rows there.
#[test]
fn a_scrolled_back_drag_selects_at_the_display_offset() {
    let (mut term, sink) = tracking_term();
    term.vt.display_offset = DisplayOffset(3);
    term.send_pointer(press(PointerButton::Left, 2, 1))
        .expect("press");
    term.send_pointer(motion(4, 2)).expect("motion");
    term.settle_writes();
    assert_eq!(sink.contents(), b"");
    assert_eq!(
        term.vt.selections,
        vec![
            SelectionOp::Start(
                GridPoint {
                    line: GridLine(-3),
                    column: GridColumn(1)
                },
                CellSide::Left,
                SelectionKind::Simple
            ),
            SelectionOp::Extend(
                GridPoint {
                    line: GridLine(-2),
                    column: GridColumn(3)
                },
                CellSide::Left
            ),
        ]
    );
    assert!(term.vt.scrolls.is_empty());
}

/// Asserts that a selection change a pointer event makes arms the
/// coalescer, so the highlight repaints without any output.
///
/// Case: the user starts a selection at an idle shell prompt, where no
/// output would otherwise repaint the pane.
#[test]
fn a_pointer_selection_change_arms_the_coalescer() {
    let (mut term, _sink) = detached_term();
    term.vt.selection_changes = true;
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("press");
    assert!(term.coalescer.is_armed());
}

/// Asserts that scrolling the viewport under a held selection drag moves
/// the selection's end onto the grid point now under the pointer.
///
/// Case: the user drags a selection at a shell prompt and spins the wheel
/// back three rows without moving the mouse.
#[test]
fn a_viewport_scroll_moves_a_held_drag_end() {
    let (mut term, _sink) = detached_term();
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("press");
    term.send_pointer(motion(5, 2)).expect("motion");
    term.vt.scroll_moves = true;
    term.vt.display_offset = DisplayOffset(3);
    term.scroll(Scroll::Delta(3));
    assert_eq!(
        term.vt.selections.last(),
        Some(&SelectionOp::Extend(
            GridPoint {
                line: GridLine(-2),
                column: GridColumn(4)
            },
            CellSide::Left
        ))
    );
}

/// Asserts that a cancel after the pane shrank reports its release at a
/// cell inside the new grid.
///
/// Case: the user holds the button over nvim near the pane's bottom-right
/// corner, a split shrinks the pane, and the window then loses focus.
#[test]
fn a_cancel_after_a_shrink_reports_inside_the_grid() {
    let (mut term, sink) = tracking_term();
    term.send_pointer(press(PointerButton::Left, 70, 20))
        .expect("press");
    term.vt.grid_size = GridSize { cols: 40, rows: 10 };
    term.send_pointer(event(PointerKind::Cancel, None, 1, 1))
        .expect("cancel");
    term.settle_writes();
    assert_eq!(sink.contents(), b"\x1b[<0;70;20M\x1b[<0;40;10m");
}

/// Asserts that a cell past the grid is clamped to the last column and
/// row before it reaches the VT.
///
/// Case: a resize shrank the pane after the host hit-tested a double click
/// near its old bottom-right corner.
#[test]
fn an_out_of_grid_cell_is_clamped_to_the_grid() {
    let (mut term, _sink) = detached_term();
    let mut input = press(PointerButton::Left, 500, 99);
    input.click_count = 2;
    term.send_pointer(input).expect("send_pointer");
    assert_eq!(
        term.vt.selections,
        vec![SelectionOp::Start(
            GridPoint {
                line: GridLine(23),
                column: GridColumn(79)
            },
            CellSide::Left,
            SelectionKind::Simple
        )]
    );
}

/// Asserts that releasing a selection drag returns the selected text, and
/// that an empty selection returns nothing.
///
/// Case: the user drags across a word at a shell prompt and lets go, then
/// does the same across blank cells.
#[test]
fn a_finished_drag_returns_the_selected_text() {
    let (mut term, _sink) = detached_term();
    term.vt.selected_text = Some("hello".to_string());
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("press");
    term.send_pointer(motion(5, 1)).expect("motion");
    assert_eq!(
        term.send_pointer(release(PointerButton::Left, 5, 1))
            .expect("release"),
        Some("hello".to_string())
    );

    term.vt.selected_text = Some(String::new());
    term.send_pointer(press(PointerButton::Left, 1, 1))
        .expect("press");
    term.send_pointer(motion(5, 1)).expect("motion");
    assert_eq!(
        term.send_pointer(release(PointerButton::Left, 5, 1))
            .expect("release"),
        None
    );
}
