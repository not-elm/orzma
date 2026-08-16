//! `TerminalGridPlugin` — applies snapshots and deltas to the per-entity
//! `TerminalGrid` Component via two `EntityEvent` observers.

use crate::schema::{
    FrameDelta, FrameSnapshot, GridCell, GridColumn, GridLine, GridPoint, Hyperlink, Run,
    TerminalGrid,
};
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
        .enumerate()
        .map(|(row, contents)| {
            runs_to_cells(
                &contents.runs,
                GridLine(row as i32 - snap.display_offset as i32),
                &snap.hyperlinks,
            )
        })
        .collect();
}

fn apply_delta(delta: On<FrameDelta>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(delta.entity) else {
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
            grid.cells[row_idx] = runs_to_cells(
                &dirty.runs,
                GridLine(i32::from(dirty.row) - delta.display_offset as i32),
                &delta.hyperlinks,
            );
        }
    }
}

fn runs_to_cells(runs: &[Run], line: GridLine, hyperlinks: &[Hyperlink]) -> Vec<GridCell> {
    let mut out: Vec<GridCell> = Vec::new();
    let mut column: u16 = 0;
    for run in runs {
        let hyperlink = run
            .hyperlink_id
            .and_then(|id| hyperlinks.iter().find(|h| h.id == id))
            .cloned();
        for grapheme in run.text.graphemes(true) {
            let w = grapheme.width();
            let width = if w >= 2 {
                2u8
            } else if w == 0 {
                0
            } else {
                1
            };
            out.push(GridCell {
                text: grapheme.to_string(),
                width,
                point: GridPoint {
                    line,
                    column: GridColumn(column),
                },
                fg: run.fg,
                bg: run.bg,
                style: run.style,
                hyperlink: hyperlink.clone(),
            });
            column = column.saturating_add(u16::from(width));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Color, Hyperlink, HyperlinkId, HyperlinkUri, Row};

    fn run_with_link(text: &str, hyperlink_id: Option<HyperlinkId>) -> Run {
        Run {
            cols: 1,
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: 0,
            text: text.to_string(),
            hyperlink_id,
        }
    }

    /// Asserts that a run's hyperlink id resolves against the frame's
    /// hyperlink table when cells are built, and that an id absent
    /// from the table leaves the cell unlinked.
    ///
    /// Case: a shell prints an OSC 8 link, so the emitted frame
    /// carries the id → URI table next to the row runs that reference
    /// it.
    #[test]
    fn runs_to_cells_resolves_hyperlink_ids_against_the_frame_table() {
        let runs = vec![
            run_with_link("a", Some(HyperlinkId(7))),
            run_with_link("b", Some(HyperlinkId(9))),
        ];
        let table = vec![Hyperlink {
            id: HyperlinkId(7),
            uri: HyperlinkUri::new("https://example"),
        }];
        let cells = runs_to_cells(&runs, GridLine(0), &table);
        assert_eq!(
            cells[0].hyperlink.as_ref().map(|h| h.id),
            Some(HyperlinkId(7))
        );
        assert_eq!(
            cells[0].hyperlink.as_ref().map(|h| h.uri.as_str()),
            Some("https://example")
        );
        assert!(cells[1].hyperlink.is_none());
    }

    /// Asserts that cell points carry the given line and a column walk
    /// that advances by display width.
    ///
    /// Case: a row mixes a wide CJK grapheme with ASCII text on a
    /// scrolled-back history line, so the ASCII cell's point must land
    /// after both columns of the wide character.
    #[test]
    fn runs_to_cells_assigns_points_by_display_width() {
        let cells = runs_to_cells(&[run_with_link("あb", None)], GridLine(-3), &[]);
        assert_eq!(
            cells[0].point,
            GridPoint {
                line: GridLine(-3),
                column: GridColumn(0),
            }
        );
        assert_eq!(
            cells[1].point,
            GridPoint {
                line: GridLine(-3),
                column: GridColumn(2),
            }
        );
    }

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
