//! Applies each `TtyFrameSignal`'s frame to the per-entity
//! `TerminalView` and `TerminalCells` components.

use crate::schema::{TerminalCells, TerminalView};
use bevy::prelude::*;
use bevy_orzmux::prelude::{OrzmuxPane, TtyFrameSignal};

/// Applies each signalled frame to its terminal's view and cells, and
/// makes every pane entity carry both.
///
/// Both are required components of [`OrzmuxPane`]. The plugin must be
/// added before any pane is promoted.
#[derive(Default)]
pub struct TerminalGridPlugin;

impl Plugin for TerminalGridPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<OrzmuxPane, TerminalView>()
            .add_observer(apply_frame);
    }
}

/// Applies the signalled frame to its terminal, touching each component
/// mutably only when the frame changes that component's own sections.
///
/// A frame that only moves the cursor, the viewport or the selection
/// leaves [`TerminalCells`] untouched.
///
/// A frame addressed to an entity without both components is ignored.
fn apply_frame(
    signal: On<TtyFrameSignal>,
    mut terminals: Query<(&mut TerminalView, &mut TerminalCells)>,
) {
    let Ok((view, cells)) = terminals.get_mut(signal.terminal) else {
        return;
    };
    if cells.differs_from(&signal.frame) {
        cells.into_inner().apply(&signal.frame);
    }
    if view.differs_from(&signal.frame) {
        view.into_inner().apply(&signal.frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        AnchoredPlacement, Cursor, DisplayOffset, GridColumn, GridLine, GridPoint, InstanceId,
        PlacementSize, SelectionGeometry, SelectionRange, quiet_frame,
    };
    use orzma_vt::prelude::{Frame, GridSize};
    use orzmux::prelude::PaneId;

    #[derive(Resource, Default)]
    struct ChangedCounts {
        views: usize,
        cells: usize,
    }

    fn count_changes(
        mut seen: ResMut<ChangedCounts>,
        views: Query<(), Changed<TerminalView>>,
        cells: Query<(), Changed<TerminalCells>>,
    ) {
        seen.views += views.iter().count();
        seen.cells += cells.iter().count();
    }

    /// Builds an app with the observer and one settled terminal entity,
    /// with the spawn's own change notifications already drained.
    fn app_with_terminal() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(TerminalGridPlugin)
            .init_resource::<ChangedCounts>()
            .add_systems(Update, count_changes);
        let terminal = app
            .world_mut()
            .spawn((TerminalView::settled(), TerminalCells::settled()))
            .id();
        app.update();
        *app.world_mut().resource_mut::<ChangedCounts>() = ChangedCounts::default();
        (app, terminal)
    }

    /// Asserts that a frame carrying nothing new leaves both components
    /// unchanged.
    ///
    /// Case: a frame repeats what the mirror already holds.
    #[test]
    fn a_frame_with_nothing_new_leaves_the_grid_unchanged() {
        let (mut app, terminal) = app_with_terminal();
        app.world_mut().trigger(TtyFrameSignal {
            terminal,
            frame: quiet_frame(),
        });
        app.update();
        let seen = app.world().resource::<ChangedCounts>();
        assert_eq!(seen.views, 0);
        assert_eq!(seen.cells, 0);
    }

    /// Asserts that a frame which only moves the cursor leaves the cell
    /// component untouched, so no GPU cell rebuild is triggered.
    ///
    /// Case: the user holds an arrow key in a shell prompt, and every
    /// frame carries a new cursor position over unchanged text.
    #[test]
    fn a_moved_cursor_leaves_the_cells_unchanged() {
        let (mut app, terminal) = app_with_terminal();
        let mut frame = quiet_frame();
        frame.cursor = Cursor {
            point: GridPoint {
                line: GridLine(0),
                column: GridColumn(4),
            },
            ..Cursor::default()
        };
        app.world_mut().trigger(TtyFrameSignal { terminal, frame });
        app.update();
        let seen = app.world().resource::<ChangedCounts>();
        assert_eq!(seen.views, 1, "the view moved");
        assert_eq!(seen.cells, 0, "no cell content changed");
    }

    /// Asserts that a frame which only scrolls the viewport leaves the
    /// cell component untouched.
    ///
    /// Case: the user scrolls back through history with the wheel while
    /// no program is producing output.
    #[test]
    fn a_scrolled_viewport_leaves_the_cells_unchanged() {
        let (mut app, terminal) = app_with_terminal();
        let frame = Frame {
            display_offset: DisplayOffset(7),
            ..quiet_frame()
        };
        app.world_mut().trigger(TtyFrameSignal { terminal, frame });
        app.update();
        let seen = app.world().resource::<ChangedCounts>();
        assert_eq!(seen.views, 1);
        assert_eq!(seen.cells, 0);
        assert_eq!(
            app.world()
                .get::<TerminalView>(terminal)
                .unwrap()
                .display_offset,
            7
        );
    }

    /// Asserts that a resize marks both components changed, keeping the
    /// cell rows in step with the view's column count.
    ///
    /// Case: the user drags the window wider and the VT repaints at the
    /// new size.
    #[test]
    fn a_resize_marks_both_components_changed() {
        let (mut app, terminal) = app_with_terminal();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            ..quiet_frame()
        };
        app.world_mut().trigger(TtyFrameSignal { terminal, frame });
        app.update();
        let seen = app.world().resource::<ChangedCounts>();
        assert_eq!(seen.views, 1);
        assert_eq!(seen.cells, 1);
        let cells = app.world().get::<TerminalCells>(terminal).unwrap();
        assert_eq!(cells.cells.len(), 2);
        assert!(cells.cells.iter().all(|row| row.len() == 3));
    }

    /// Asserts that a frame which only changes the selection leaves the
    /// cell component untouched.
    ///
    /// Case: the user drags the mouse across text no program is
    /// rewriting, extending the selection one cell at a time.
    #[test]
    fn a_changed_selection_leaves_the_cells_unchanged() {
        let (mut app, terminal) = app_with_terminal();
        let frame = Frame {
            selection: Some(SelectionRange {
                start: GridPoint {
                    line: GridLine(0),
                    column: GridColumn(0),
                },
                end: GridPoint {
                    line: GridLine(0),
                    column: GridColumn(0),
                },
                geometry: SelectionGeometry::Linear,
            }),
            ..quiet_frame()
        };
        app.world_mut().trigger(TtyFrameSignal { terminal, frame });
        app.update();
        let seen = app.world().resource::<ChangedCounts>();
        assert_eq!(seen.views, 1);
        assert_eq!(seen.cells, 0);
    }

    /// Asserts that a frame's placements list reaches the view through
    /// the observer.
    ///
    /// Case: a program mounts a webview and the next frame lists it.
    #[test]
    fn a_frame_delivers_its_placements() {
        let (mut app, terminal) = app_with_terminal();
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
                .get::<TerminalView>(terminal)
                .unwrap()
                .placements,
            vec![placed]
        );
    }

    /// Asserts that a frame addressed to an entity without both
    /// components is ignored rather than panicking.
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
        assert!(app.world().get::<TerminalView>(bare).is_none());
        assert!(app.world().get::<TerminalCells>(bare).is_none());
    }

    /// Asserts that a frame addressed to an entity whose `TerminalCells`
    /// was removed after spawn is ignored rather than reconstructed from
    /// the requirement, leaving the view unmutated.
    ///
    /// Case: a pane entity that already carries both components loses
    /// its cells component to a later removal, while its view stays.
    #[test]
    fn a_frame_for_an_entity_with_only_one_component_is_ignored() {
        let mut app = App::new();
        app.add_plugins(TerminalGridPlugin);
        let terminal = app.world_mut().spawn(TerminalView::settled()).id();
        app.world_mut()
            .entity_mut(terminal)
            .remove::<TerminalCells>();
        let mut frame = quiet_frame();
        frame.cursor = Cursor {
            point: GridPoint {
                line: GridLine(0),
                column: GridColumn(4),
            },
            ..Cursor::default()
        };
        app.world_mut().trigger(TtyFrameSignal { terminal, frame });
        let view = app.world().get::<TerminalView>(terminal).unwrap();
        assert_eq!(view.cursor, Some(Cursor::default()));
        assert!(app.world().get::<TerminalCells>(terminal).is_none());
    }

    /// Asserts that spawning `TerminalView` alone also inserts
    /// `TerminalCells`, rather than leaving it absent until a frame
    /// arrives.
    ///
    /// Case: code elsewhere spawns a `TerminalView` without listing
    /// `TerminalCells` alongside it.
    #[test]
    fn a_bare_terminal_view_spawn_gets_terminal_cells() {
        let mut app = App::new();
        let terminal = app.world_mut().spawn(TerminalView::default()).id();
        assert!(app.world().get::<TerminalCells>(terminal).is_some());
    }

    /// Asserts that a frame signalled at an `OrzmuxPane` entity lands in
    /// both the view and the cells the required components gave it.
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
        let view = app.world().get::<TerminalView>(terminal).unwrap();
        assert_eq!((view.cols, view.rows), (4, 3));
        let cells = app.world().get::<TerminalCells>(terminal).unwrap();
        assert_eq!(cells.cells.len(), 3);
        assert!(cells.cells.iter().all(|row| row.len() == 4));
    }
}
