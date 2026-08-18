//! Tests for [`AlacrittyVtBackend`]: shared fixtures plus one child
//! module per exercised concern.

use super::*;
use crate::damage::DamageRows;
use crate::schema::{CellSide, GridColumn, GridLine, GridPoint, SelectionGeometry};
use alacritty_terminal::index::Point as AlacPoint;

mod cursor;
mod cursor_vi;
mod damage;
mod modes_and_grid;
mod palette;
mod scroll;
mod selection;
mod selection_output;
mod selection_vi;

/// Row count of the grid every fixture in this module builds.
const GRID_ROWS: u16 = 24;
/// Column count of the grid every fixture in this module builds.
const GRID_COLS: u16 = 80;
/// Newlines a fresh grid absorbs before history starts growing.
const VIEWPORT_FILL_ROWS: usize = GRID_ROWS as usize - 1;
const SEEDED_HISTORY_ROWS: usize = 10;

fn vt_after(bytes: &[u8]) -> AlacrittyVtBackend {
    let mut vt = AlacrittyVtBackend::new(GRID_COLS, GRID_ROWS);
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

fn point(line: i32, column: u16) -> GridPoint {
    GridPoint {
        line: GridLine(line),
        column: GridColumn(column),
    }
}

fn start_simple(vt: &mut AlacrittyVtBackend, x: u16, line: i32) {
    vt.start_selection(point(line, x), CellSide::Left, SelectionKind::Simple)
        .unwrap();
}

fn update_to(vt: &mut AlacrittyVtBackend, x: u16, line: i32, side: CellSide) {
    vt.update_selection(point(line, x), side).unwrap();
}
