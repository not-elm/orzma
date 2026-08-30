//! `TerminalGridPlugin` — applies each `TtyFrameSignal`'s frame to the
//! per-entity `TerminalGrid` component through one `EntityEvent`
//! observer.

use crate::schema::TerminalGrid;
use bevy::prelude::*;
use bevy_orzma_tty::prelude::TtyFrameSignal;

/// Registers the `apply_frame` observer.
#[derive(Default)]
pub struct TerminalGridPlugin;

impl Plugin for TerminalGridPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_frame);
    }
}

/// Applies the signalled frame to its terminal's grid, touching the
/// component mutably only when the frame changes something.
///
/// `orzma_tty` emits at most one frame per coalesce window, and
/// `FrameTracker::emit` already returns `None` when nothing changed, so
/// a signalled frame is usually real damage. The gate here is what
/// keeps the mirror honest on the frames that are not: a frame naming
/// only rows outside the mirror's range, only hyperlink ids the mirror
/// already knows, or sections that already equal the mirror's own.
/// `update_terminal_material` reads a changed grid as a reason to
/// rebuild the GPU buffers, so a spurious write here would rebuild
/// them for nothing.
///
/// A terminal entity must carry a `TerminalGrid` from the same spawn
/// as its handle: a frame delivered to an entity without one is
/// dropped silently, and `orzma_tty` offers no repaint request to
/// recover the bootstrap frame that was lost.
fn apply_frame(signal: On<TtyFrameSignal>, mut terminals: Query<&mut TerminalGrid>) {
    let Ok(grid) = terminals.get_mut(signal.terminal) else {
        return;
    };
    if grid.differs_from(&signal.frame) {
        grid.into_inner().apply(&signal.frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        AnchoredPlacement, Cursor, DisplayOffset, GridColumn, GridLine, GridPoint, PlacementId,
        PlacementSize,
    };
    use bevy_orzma_tty::prelude::{OrzmaTtyHandle, OrzmaTtyPlugin};
    use orzma_vt::prelude::{Frame, GridSize};
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct ChangedGrids(usize);

    fn count_changed_grids(
        mut seen: ResMut<ChangedGrids>,
        grids: Query<(), Changed<TerminalGrid>>,
    ) {
        seen.0 += grids.iter().count();
    }

    /// A frame for a one-by-one grid that changes nothing on its own.
    fn quiet_frame() -> Frame {
        Frame {
            size: GridSize { cols: 1, rows: 1 },
            rows: vec![],
            cursor: Cursor::default(),
            display_offset: DisplayOffset(0),
            vi_cursor: None,
            selection: None,
            placements: None,
            palette: None,
            hyperlinks: vec![],
        }
    }

    /// Builds an app with the observer and one settled grid entity,
    /// with the spawn's own change notification already drained.
    fn app_with_grid() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(TerminalGridPlugin)
            .init_resource::<ChangedGrids>()
            .add_systems(Update, count_changed_grids);
        let terminal = app
            .world_mut()
            .spawn(TerminalGrid {
                cols: 1,
                rows: 1,
                cells: vec![vec![]],
                cursor: Some(Cursor::default()),
                ..Default::default()
            })
            .id();
        app.update();
        app.world_mut().resource_mut::<ChangedGrids>().0 = 0;
        (app, terminal)
    }

    /// Asserts that a frame carrying nothing new leaves the grid
    /// component unchanged.
    ///
    /// Case: a frame's every section already equals what the mirror
    /// holds, because the coalescer folded in a change the mirror had
    /// already settled to before this frame reached it.
    #[test]
    fn a_frame_with_nothing_new_leaves_the_grid_unchanged() {
        let (mut app, terminal) = app_with_grid();
        app.world_mut().trigger(TtyFrameSignal {
            terminal,
            frame: quiet_frame(),
        });
        app.update();
        assert_eq!(app.world().resource::<ChangedGrids>().0, 0);
    }

    /// Asserts that a frame whose metadata moved does mark the grid
    /// changed.
    ///
    /// Case: the user scrolls back through history without the shell
    /// repainting any cell.
    #[test]
    fn a_frame_that_moves_the_viewport_marks_the_grid_changed() {
        let (mut app, terminal) = app_with_grid();
        app.world_mut().trigger(TtyFrameSignal {
            terminal,
            frame: Frame {
                display_offset: DisplayOffset(7),
                ..quiet_frame()
            },
        });
        app.update();
        assert_eq!(app.world().resource::<ChangedGrids>().0, 1);
        assert_eq!(
            app.world()
                .get::<TerminalGrid>(terminal)
                .unwrap()
                .display_offset,
            7
        );
    }

    /// Asserts that a frame's placements list reaches the grid through
    /// the observer.
    ///
    /// Case: a program mounts a webview and the next frame lists it.
    #[test]
    fn a_frame_delivers_its_placements() {
        let (mut app, terminal) = app_with_grid();
        let placed = AnchoredPlacement {
            id: PlacementId(1),
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 4, cols: 5 },
        };
        app.world_mut().trigger(TtyFrameSignal {
            terminal,
            frame: Frame {
                placements: Some(vec![placed]),
                ..quiet_frame()
            },
        });
        assert_eq!(
            app.world()
                .get::<TerminalGrid>(terminal)
                .unwrap()
                .placements,
            vec![placed]
        );
    }

    /// Asserts that a frame addressed to a terminal without a grid is
    /// ignored rather than panicking.
    ///
    /// Case: the host spawns the handle a frame before it attaches the
    /// renderer's grid component.
    #[test]
    fn a_frame_for_an_entity_without_a_grid_is_ignored() {
        let mut app = App::new();
        app.add_plugins(TerminalGridPlugin);
        let bare = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TtyFrameSignal {
            terminal: bare,
            frame: quiet_frame(),
        });
        assert!(app.world().get::<TerminalGrid>(bare).is_none());
    }

    /// Asserts that bytes fed to a terminal handle reach its grid
    /// through the signal pump and the frame observer.
    ///
    /// Case: the shell prints its first prompt after the terminal
    /// spawns.
    #[test]
    fn fed_bytes_reach_the_grid_through_the_pump() {
        let mut app = App::new();
        app.add_plugins((OrzmaTtyPlugin, TerminalGridPlugin));
        let (mut handle, _sink) = OrzmaTtyHandle::detached(4, 3);
        handle.feed_bytes(b"hi");
        let terminal = app
            .world_mut()
            .spawn((handle, TerminalGrid::default()))
            .id();
        // NOTE: The coalescer decides on wall-clock time — 3 ms of
        // idle after the last chunk, 12 ms at most — so the pump must
        // run after that window closed or the frame is still pending.
        std::thread::sleep(Duration::from_millis(20));
        app.update();

        let grid = app.world().get::<TerminalGrid>(terminal).unwrap();
        assert_eq!((grid.cols, grid.rows), (4, 3));
        assert_eq!(grid.cells[0][0].text, "h");
        assert_eq!(grid.cells[0][1].text, "i");
    }
}
