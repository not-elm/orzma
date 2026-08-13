//! Tests for [`AlacrittyVtBackend`]: shared fixtures plus one child
//! module per exercised concern.

use super::*;
use crate::schema::{CellSide, SelectionGeometry};
use alacritty_terminal::index::Point as AlacPoint;

mod damage;
mod modes_and_grid;
mod scroll;
mod selection;
mod selection_output;
mod selection_vi;

const VIEWPORT_FILL_ROWS: usize = 23;
const SEEDED_HISTORY_ROWS: usize = 10;

/// Row count of the grid every fixture in this module builds.
const GRID_ROWS: u16 = 24;

fn vt_after(bytes: &[u8]) -> AlacrittyVtBackend {
    let mut vt = AlacrittyVtBackend::new(80, 24);
    vt.interpret(bytes);
    vt
}

// NOTE: alacritty pushes a row into history only once the cursor already
// sits on the last screen line, so the first `VIEWPORT_FILL_ROWS` newlines
// of a 24-row grid fill the viewport without growing `history_size`. The
// precondition assert keeps a change in that accounting from silently
// collapsing every `display_offset` expectation below to zero.
fn vt_with_history(history_rows: usize) -> AlacrittyVtBackend {
    let bytes: Vec<u8> = (0..history_rows + VIEWPORT_FILL_ROWS)
        .flat_map(|i| format!("l{i}\r\n").into_bytes())
        .collect();
    let vt = vt_after(&bytes);
    assert_eq!(vt.term.grid().history_size(), history_rows);
    vt
}

// NOTE: a fresh `Term` starts fully damaged for the bootstrap paint, so a
// test that wants to observe only what its own bytes staged must clear
// both halves — the staged value AND alacritty's accumulator. Stands in
// for `frames()`, which is still `todo!()`.
fn drain_staged(vt: &mut AlacrittyVtBackend) {
    vt.pending_damage = None;
    vt.term.reset_damage();
}

fn cell(x: u16, y: i16) -> ViewportPoint {
    ViewportPoint { row: y, column: x }
}

fn start_simple(vt: &mut AlacrittyVtBackend, x: u16, y: i16) {
    vt.apply_selection(SelectionOp::StartAt {
        cell: cell(x, y),
        side: CellSide::Left,
        kind: SelectionKind::Simple,
    })
    .unwrap();
}

fn update_to(vt: &mut AlacrittyVtBackend, x: u16, y: i16, side: CellSide) {
    vt.apply_selection(SelectionOp::UpdateTo {
        cell: cell(x, y),
        side,
    })
    .unwrap();
}
