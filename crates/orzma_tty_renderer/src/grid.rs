//! `TerminalGridPlugin` — applies snapshots and deltas to the per-entity
//! `TerminalGrid` Component via two `EntityEvent` observers.

use crate::schema::{Cell, FrameDelta, FrameSnapshot, Run, TerminalGrid};
use bevy::prelude::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Registers the `apply_snapshot` and `apply_delta` observers.
#[derive(Default)]
pub struct TerminalGridPlugin;

impl Plugin for TerminalGridPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_snapshot).add_observer(apply_delta);
    }
}

fn apply_snapshot(snap: On<FrameSnapshot>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(snap.entity) else {
        debug!(
            entity = ?snap.entity,
            cols = snap.cols,
            rows = snap.rows,
            "FrameSnapshot dropped: entity has no TerminalGrid (render bundle not yet injected)"
        );
        return;
    };
    grid.cols = snap.cols;
    grid.rows = snap.rows;
    grid.cursor = Some(snap.cursor.clone());
    grid.display_offset = snap.display_offset;
    grid.history_size = snap.history_size;
    grid.history_base = snap.history_base;
    grid.last_seq = snap.seq;
    grid.modes = snap.modes.clone();
    grid.hyperlinks.clear();
    grid.hyperlinks
        .extend(snap.hyperlinks.iter().map(|h| (h.id, h.uri.clone())));
    grid.vi_cursor = snap.vi_cursor;
    grid.selection = snap.selection;
    grid.default_bg = snap.default_bg;
    grid.cells = snap
        .rows_data
        .iter()
        .map(|row| runs_to_cells(&row.runs))
        .collect();
}

fn apply_delta(delta: On<FrameDelta>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(delta.entity) else {
        debug!(
            entity = ?delta.entity,
            "FrameDelta dropped: entity has no TerminalGrid (render bundle not yet injected)"
        );
        return;
    };
    grid.cursor = Some(delta.cursor.clone());
    grid.display_offset = delta.display_offset;
    grid.history_size = delta.history_size;
    grid.history_base = delta.history_base;
    grid.last_seq = delta.seq;
    grid.vi_cursor = delta.vi_cursor;
    grid.selection = delta.selection;
    for h in &delta.hyperlinks {
        if !grid.hyperlinks.iter().any(|(id, _)| *id == h.id) {
            grid.hyperlinks.push((h.id, h.uri.clone()));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Hyperlink, HyperlinkId, HyperlinkUri, Row};

    fn grid_with(seed: Vec<(HyperlinkId, HyperlinkUri)>) -> TerminalGrid {
        TerminalGrid {
            cols: 1,
            rows: 1,
            cells: vec![vec![]],
            hyperlinks: seed,
            ..Default::default()
        }
    }

    #[test]
    fn apply_snapshot_clears_and_extends_hyperlinks() {
        let mut app = App::new();
        app.add_observer(apply_snapshot);
        let entity = app
            .world_mut()
            .spawn(grid_with(vec![(HyperlinkId(99), HyperlinkUri::new("old"))]))
            .id();
        app.world_mut().trigger(FrameSnapshot {
            entity,
            seq: 1,
            cols: 1,
            rows: 1,
            cursor: Default::default(),
            rows_data: vec![Row { runs: vec![] }],
            reason: Default::default(),
            modes: vec![],
            hyperlinks: vec![Hyperlink {
                id: HyperlinkId(1),
                uri: HyperlinkUri::new("https://new"),
            }],
            display_offset: 0,
            history_size: 0,
            history_base: 0,
            vi_cursor: None,
            selection: None,
            default_bg: [0, 0, 0],
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.hyperlinks.len(), 1);
        assert_eq!(grid.hyperlinks[0].0, HyperlinkId(1));
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://new");
    }

    #[test]
    fn apply_delta_mirrors_history_fields() {
        let mut app = App::new();
        app.add_observer(apply_snapshot).add_observer(apply_delta);
        let entity = app.world_mut().spawn(grid_with(vec![])).id();
        app.world_mut().trigger(FrameSnapshot {
            entity,
            seq: 1,
            cols: 1,
            rows: 1,
            cursor: Default::default(),
            rows_data: vec![Row { runs: vec![] }],
            reason: Default::default(),
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 7,
            history_base: 3,
            vi_cursor: None,
            selection: None,
            default_bg: [0, 0, 0],
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.history_size, 7);
        assert_eq!(grid.history_base, 3);
        app.world_mut().trigger(FrameDelta {
            entity,
            seq: 2,
            cursor: Default::default(),
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 9,
            history_base: 5,
            vi_cursor: None,
            selection: None,
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.history_size, 9);
        assert_eq!(grid.history_base, 5);
    }

    #[test]
    fn apply_delta_merges_hyperlinks_without_overwrite() {
        let mut app = App::new();
        app.add_observer(apply_delta);
        let entity = app
            .world_mut()
            .spawn(grid_with(vec![(
                HyperlinkId(1),
                HyperlinkUri::new("https://old"),
            )]))
            .id();
        app.world_mut().trigger(FrameDelta {
            entity,
            seq: 2,
            cursor: Default::default(),
            dirty_rows: vec![],
            hyperlinks: vec![
                Hyperlink {
                    id: HyperlinkId(1),
                    uri: HyperlinkUri::new("https://CHANGED"),
                },
                Hyperlink {
                    id: HyperlinkId(2),
                    uri: HyperlinkUri::new("https://new"),
                },
            ],
            display_offset: 0,
            history_size: 0,
            history_base: 0,
            vi_cursor: None,
            selection: None,
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.hyperlinks.len(), 2);
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://old");
        assert_eq!(grid.hyperlinks[1].1.as_str(), "https://new");
    }
}
