//! GridPlugin — applies snapshots and deltas to `Resource<TerminalGrid>`.
//!
//! Reads `FrameReceived` messages produced by `WirePlugin`, expands run
//! text into per-grapheme cells via `unicode-segmentation` +
//! `unicode-width`, and stores the result for downstream rendering plugins.

use crate::schema::{Cell, FrameDelta, FrameSnapshot, Run, TerminalGrid};
use bevy::{ecs::event::Trigger, prelude::*};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Plugin: maintains `TerminalGrid` from `FrameReceived`.
#[derive(Default)]
pub struct TerminalGridPlugin;

impl Plugin for TerminalGridPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_snapshot).add_observer(apply_delta);
    }
}

fn apply_snapshot(snap: On<FrameSnapshot>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(snap.entity) else {
        return;
    };
    grid.cols = snap.cols;
    grid.rows = snap.rows;
    grid.cursor = Some(snap.cursor.clone());
    grid.display_offset = snap.display_offset;
    grid.history_size = snap.history_size;
    grid.last_seq = snap.seq;
    grid.modes = snap.modes.clone();
    grid.vi_cursor = snap.vi_cursor;
    grid.selection = snap.selection;
    grid.cells = snap
        .rows_data
        .iter()
        .map(|row| runs_to_cells(&row.runs))
        .collect();
}

fn apply_delta(delta: On<FrameDelta>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(delta.entity) else {
        return;
    };
    grid.cursor = Some(delta.cursor.clone());
    grid.display_offset = delta.display_offset;
    grid.last_seq = delta.seq;
    grid.vi_cursor = delta.vi_cursor;
    grid.selection = delta.selection;
    for dirty in &delta.dirty_rows {
        let row_idx = dirty.row as usize;
        if row_idx < grid.cells.len() {
            grid.cells[row_idx] = runs_to_cells(&dirty.runs);
        }
    }
}

fn runs_to_cells(runs: &[Run]) -> Vec<Cell> {
    let mut out: Vec<Cell> = Vec::new();
    for run in runs {
        for grapheme in run.text.graphemes(true) {
            let w = grapheme.width();
            let width = if w >= 2 {
                2u8
            } else if w == 0 {
                0
            } else {
                1
            };
            out.push(Cell {
                text: grapheme.to_string(),
                width,
                fg: run.fg,
                bg: run.bg,
                style: run.style,
                hyperlink_id: run.hyperlink_id,
            });
        }
    }
    out
}

// /// Decoded shape of a `kind: "mode"` text frame.
// #[derive(serde::Deserialize)]
// struct ModeFrame {
//     kind: String,
//     added: Vec<String>,
//     removed: Vec<String>,
// }
//
// fn apply_mode_changes(mut grid: ResMut<TerminalGrid>, mut texts: MessageReader<TextFrame>) {
//     for TextFrame(s) in texts.read() {
//         let Ok(parsed) = serde_json::from_str::<ModeFrame>(s) else {
//             continue;
//         };
//         if parsed.kind != "mode" {
//             continue;
//         }
//         for r in &parsed.removed {
//             if let Some(pos) = grid.modes.iter().position(|m| m == r) {
//                 grid.modes.swap_remove(pos);
//             }
//         }
//         for a in parsed.added {
//             if !grid.modes.contains(&a) {
//                 grid.modes.push(a);
//             }
//         }
//     }
// }
