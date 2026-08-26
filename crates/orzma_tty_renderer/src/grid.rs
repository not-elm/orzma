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
    grid.cursor = Some(snap.cursor);
    grid.display_offset = snap.display_offset;
    grid.hyperlinks.clear();
    grid.hyperlinks
        .extend(snap.hyperlinks.iter().map(|h| (h.id, h.uri.clone())));
    grid.vi_cursor = snap.vi_cursor;
    grid.selection = snap.selection;
    grid.palette = snap.palette.clone();
    grid.placements.clone_from(&snap.placements);
    grid.cells = snap
        .rows_data
        .iter()
        .enumerate()
        .map(|(row, contents)| {
            runs_to_cells(
                contents,
                GridLine(row as i32 - snap.display_offset as i32),
                &snap.hyperlinks,
            )
        })
        .collect();
}

// NOTE: Every write here is guarded because a delta can legitimately
// carry nothing to draw — the VT emits one whenever a damage cycle ran,
// including while the user is scrolled far enough back that the output
// is off-window. Assigning identical values would still deref
// mutably, and `update_terminal_material` reads that as "this grid
// changed" and rebuilds the GPU buffers for a screen nothing moved on.
fn apply_delta(delta: On<FrameDelta>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(mut grid) = terminals.get_mut(delta.entity) else {
        return;
    };
    if grid.cursor != Some(delta.cursor) {
        grid.cursor = Some(delta.cursor);
    }
    if grid.display_offset != delta.display_offset {
        grid.display_offset = delta.display_offset;
    }
    if grid.vi_cursor != delta.vi_cursor {
        grid.vi_cursor = delta.vi_cursor;
    }
    if grid.selection != delta.selection {
        grid.selection = delta.selection;
    }
    if grid.placements != delta.placements {
        grid.placements.clone_from(&delta.placements);
    }
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
                style: run.style.bits(),
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
    use crate::schema::{
        AnchoredPlacement, Color, Cursor, GridColumn, Hyperlink, HyperlinkId, HyperlinkUri,
        Palette, PlacementId, PlacementSize, Rgb, Row, Style,
    };

    fn run_with_link(text: &str, hyperlink_id: Option<HyperlinkId>) -> Run {
        Run {
            cols: 1,
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: Style::empty(),
            text: text.to_string(),
            hyperlink_id,
        }
    }

    #[derive(Resource, Default)]
    struct ChangedGrids(usize);

    fn count_changed_grids(
        mut seen: ResMut<ChangedGrids>,
        grids: Query<(), Changed<TerminalGrid>>,
    ) {
        seen.0 += grids.iter().count();
    }

    /// Asserts that a delta carrying nothing new leaves the grid
    /// component unchanged.
    ///
    /// The agreed policy guards every write rather than assigning
    /// unconditionally: a delta is emitted whenever a damage cycle ran,
    /// so one arrives per PTY chunk even while the user is scrolled far
    /// enough back that the output is off-window. An unconditional
    /// assignment would still mark the component changed, and
    /// `update_terminal_material` reads that as a reason to rebuild the
    /// GPU buffers.
    ///
    /// Case: the user reads scrollback while a build keeps printing at
    /// the live tail.
    #[test]
    fn a_delta_with_nothing_new_leaves_the_grid_unchanged() {
        let mut app = App::new();
        app.add_observer(apply_delta)
            .init_resource::<ChangedGrids>()
            .add_systems(Update, count_changed_grids);
        let entity = app
            .world_mut()
            .spawn(TerminalGrid {
                cursor: Some(Cursor::default()),
                ..grid_with(vec![])
            })
            .id();
        app.update();
        app.world_mut().resource_mut::<ChangedGrids>().0 = 0;

        app.world_mut().trigger(FrameDelta {
            entity,
            cursor: Cursor::default(),
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            vi_cursor: None,
            selection: None,
            placements: vec![],
        });
        app.update();

        assert_eq!(app.world().resource::<ChangedGrids>().0, 0);
    }

    /// Asserts that a delta whose metadata moved does mark the grid
    /// changed.
    ///
    /// Case: the user presses an arrow key and the application moves
    /// the caret without repainting a cell.
    #[test]
    fn a_delta_that_moves_the_cursor_marks_the_grid_changed() {
        let mut app = App::new();
        app.add_observer(apply_delta)
            .init_resource::<ChangedGrids>()
            .add_systems(Update, count_changed_grids);
        let entity = app
            .world_mut()
            .spawn(TerminalGrid {
                cursor: Some(Cursor::default()),
                ..grid_with(vec![])
            })
            .id();
        app.update();
        app.world_mut().resource_mut::<ChangedGrids>().0 = 0;

        app.world_mut().trigger(FrameDelta {
            entity,
            cursor: Cursor::default(),
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 7,
            vi_cursor: None,
            selection: None,
            placements: vec![],
        });
        app.update();

        assert_eq!(app.world().resource::<ChangedGrids>().0, 1);
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
            cols: 1,
            rows: 1,
            cursor: Default::default(),
            rows_data: vec![Row::from(vec![])],
            hyperlinks: vec![Hyperlink {
                id: HyperlinkId(1),
                uri: HyperlinkUri::new("https://new"),
            }],
            display_offset: 0,
            vi_cursor: None,
            selection: None,
            placements: vec![],
            palette: Palette::default(),
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.hyperlinks.len(), 1);
        assert_eq!(grid.hyperlinks[0].0, HyperlinkId(1));
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://new");
    }

    /// Asserts that a delta's placements list replaces the mirror
    /// wholesale, including down to empty.
    ///
    /// The list is declarative — absence means "not visible this
    /// frame" — so a merge would keep stale rectangles alive.
    ///
    /// Case: a webview scrolls out of the viewport, so the next delta
    /// carries an empty placements list while the rect stays mounted.
    #[test]
    fn apply_delta_replaces_placements_wholesale() {
        let mut app = App::new();
        app.add_observer(apply_delta);
        let entity = app.world_mut().spawn(grid_with(vec![])).id();
        let placed = AnchoredPlacement {
            id: PlacementId(1),
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 4, cols: 5 },
        };
        app.world_mut().trigger(FrameDelta {
            entity,
            cursor: Default::default(),
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            vi_cursor: None,
            selection: None,
            placements: vec![placed],
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.placements, vec![placed]);
        app.world_mut().trigger(FrameDelta {
            entity,
            cursor: Default::default(),
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            vi_cursor: None,
            selection: None,
            placements: vec![],
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.placements, vec![]);
    }

    /// Asserts that a snapshot replaces the grid's palette mirror.
    ///
    /// Case: OSC 4 recolors a palette slot, and the repaint that
    /// follows arrives as a full snapshot.
    #[test]
    fn apply_snapshot_replaces_the_palette() {
        let mut app = App::new();
        app.add_observer(apply_snapshot);
        let entity = app.world_mut().spawn(grid_with(vec![])).id();
        let palette = Palette {
            background: Rgb { r: 9, g: 8, b: 7 },
            ..Palette::default()
        };
        app.world_mut().trigger(FrameSnapshot {
            entity,
            cols: 1,
            rows: 1,
            cursor: Default::default(),
            rows_data: vec![Row::from(vec![])],
            hyperlinks: vec![],
            display_offset: 0,
            vi_cursor: None,
            selection: None,
            placements: vec![],
            palette,
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.palette.background, Rgb { r: 9, g: 8, b: 7 });
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
            placements: vec![],
        });
        app.update();
        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert_eq!(grid.hyperlinks.len(), 2);
        assert_eq!(grid.hyperlinks[0].1.as_str(), "https://old");
        assert_eq!(grid.hyperlinks[1].1.as_str(), "https://new");
    }
}
