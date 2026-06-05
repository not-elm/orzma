//! `TerminalGridPlugin` — applies snapshots and deltas to the per-entity
//! `TerminalGrid` Component via two `EntityEvent` observers.

use crate::schema::{Cell, RgbaColor, Run, TerminalDelta, TerminalGrid, TerminalSnapshot};
use bevy::prelude::{App, Color, On, Plugin, Query};
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

fn apply_snapshot(ev: On<TerminalSnapshot>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(ev.entity) else {
        return;
    };
    grid.cols = ev.snapshot.cols;
    grid.rows = ev.snapshot.rows;
    grid.cursor = Some(ev.snapshot.cursor.clone());
    grid.display_offset = ev.snapshot.display_offset;
    grid.history_size = ev.snapshot.history_size;
    grid.last_seq = ev.snapshot.seq;
    grid.modes = ev.snapshot.modes.clone();
    grid.hyperlinks.clear();
    grid.hyperlinks
        .extend(ev.snapshot.hyperlinks.iter().map(|h| (h.id, h.uri.clone())));
    grid.vi_cursor = ev.snapshot.vi_cursor;
    grid.selection = ev.snapshot.selection;
    grid.cells = ev
        .snapshot
        .rows_data
        .iter()
        .map(|row| runs_to_cells(&row.runs))
        .collect();
}

fn apply_delta(ev: On<TerminalDelta>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(ev.entity) else {
        return;
    };
    grid.cursor = Some(ev.delta.cursor.clone());
    grid.display_offset = ev.delta.display_offset;
    grid.last_seq = ev.delta.seq;
    grid.vi_cursor = ev.delta.vi_cursor;
    grid.selection = ev.delta.selection;
    for h in &ev.delta.hyperlinks {
        if !grid.hyperlinks.iter().any(|(id, _)| *id == h.id) {
            grid.hyperlinks.push((h.id, h.uri.clone()));
        }
    }
    for dirty in &ev.delta.dirty_rows {
        let row_idx = dirty.row as usize;
        if row_idx < grid.cells.len() {
            grid.cells[row_idx] = runs_to_cells(&dirty.runs);
        }
    }
}

fn rgba_to_color(c: RgbaColor) -> Color {
    Color::srgba_u8(c.r, c.g, c.b, c.a)
}

fn runs_to_cells(runs: &[Run]) -> Vec<Cell> {
    let mut out: Vec<Cell> = Vec::new();
    for run in runs {
        let fg = rgba_to_color(run.fg);
        let bg = rgba_to_color(run.bg);
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
                fg,
                bg,
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
    use ozmux_vt::frame::{FrameDelta, FrameSnapshot};

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
        app.world_mut().trigger(TerminalSnapshot {
            entity,
            snapshot: FrameSnapshot {
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
                vi_cursor: None,
                selection: None,
            },
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.hyperlinks.len(), 1);
        assert_eq!(grid.hyperlinks[0].0, HyperlinkId(1));
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://new");
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
        app.world_mut().trigger(TerminalDelta {
            entity,
            delta: FrameDelta {
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
                vi_cursor: None,
                selection: None,
            },
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.hyperlinks.len(), 2);
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://old");
        assert_eq!(grid.hyperlinks[1].1.as_str(), "https://new");
    }
}
