//! Applies each `TtyFrameSignal`'s frame to the per-entity
//! `TerminalGrid` component.

use crate::schema::TerminalGrid;
use bevy::prelude::*;
use bevy_orzmux::prelude::{OrzmuxPane, TtyFrameSignal};

/// Applies each signalled frame to its terminal's grid, and makes every
/// pane entity carry a `TerminalGrid`.
///
/// The grid is a required component of [`OrzmuxPane`]. The plugin must be
/// added before any pane is promoted.
#[derive(Default)]
pub struct TerminalGridPlugin;

impl Plugin for TerminalGridPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<OrzmuxPane, TerminalGrid>()
            .add_observer(apply_frame);
    }
}

/// Applies the signalled frame to its terminal's grid, touching the
/// component mutably only when the frame changes something.
///
/// A frame naming only rows outside the mirror's range, only hyperlink
/// ids the mirror already knows, or sections that already equal the
/// mirror's own leaves the component untouched.
///
/// A frame addressed to an entity without a grid is ignored.
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
        AnchoredPlacement, DisplayOffset, GridColumn, GridLine, GridPoint, InstanceId,
        PlacementSize, quiet_frame,
    };
    use orzma_vt::prelude::{Frame, GridSize};
    use orzmux::prelude::PaneId;

    #[derive(Resource, Default)]
    struct ChangedGrids(usize);

    fn count_changed_grids(
        mut seen: ResMut<ChangedGrids>,
        grids: Query<(), Changed<TerminalGrid>>,
    ) {
        seen.0 += grids.iter().count();
    }

    /// Builds an app with the observer and one settled grid entity,
    /// with the spawn's own change notification already drained.
    fn app_with_grid() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(TerminalGridPlugin)
            .init_resource::<ChangedGrids>()
            .add_systems(Update, count_changed_grids);
        let terminal = app.world_mut().spawn(TerminalGrid::settled()).id();
        app.update();
        app.world_mut().resource_mut::<ChangedGrids>().0 = 0;
        (app, terminal)
    }

    /// Asserts that a frame carrying nothing new leaves the grid
    /// component unchanged.
    ///
    /// Case: a frame repeats what the mirror already holds.
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
    /// Case: a frame that only moves the display offset reaches a settled
    /// mirror.
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
            id: InstanceId(1),
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

    /// Asserts that a frame addressed to an entity without a grid is
    /// ignored rather than panicking.
    ///
    /// Case: a signal names an entity that never carried a terminal
    /// handle.
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

    /// Asserts that a frame signalled at an `OrzmuxPane` entity lands in the
    /// grid the required component gave it.
    ///
    /// Case: the backend sends a pane's bootstrap frame right after the
    /// GUI promoted its entity.
    #[test]
    fn a_signalled_frame_reaches_the_required_grid() {
        let mut app = App::new();
        app.add_plugins(TerminalGridPlugin);
        let terminal = app.world_mut().spawn(OrzmuxPane(PaneId(1))).id();
        let mut frame = quiet_frame();
        frame.size = GridSize { cols: 4, rows: 3 };
        app.world_mut().trigger(TtyFrameSignal { terminal, frame });
        let grid = app.world().get::<TerminalGrid>(terminal).unwrap();
        assert_eq!((grid.cols, grid.rows), (4, 3));
    }
}
