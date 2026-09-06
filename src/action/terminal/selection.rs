//! Local-selection actions: start / update / clear a selection on a terminal
//! surface, and copy the current selection to the clipboard.

use crate::action::clipboard::CopyAction;
use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use bevy_orzma_mux::prelude::{
    CellSide, GridPoint, RequestTtyCopySelection, RequestTtySelectionClear,
    RequestTtySelectionStart, RequestTtySelectionUpdate, SelectionKind, TtySelectionTextSignal,
};
use orzma_tty_renderer::schema::TerminalGrid;
use orzma_vt::prelude::{DisplayOffset, ViewportLine};

/// Starts a new local selection on `entity` at `point`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionStart {
    /// The terminal entity to start the selection on.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative anchor of the new selection: line `0` is the
    /// top row of the *displayed* viewport, not the active grid.
    pub point: GridPoint,
    /// Which half of the cell the anchor sits in.
    pub side: CellSide,
    /// The selection granularity (simple / lines).
    pub ty: SelectionKind,
}

/// Extends `entity`'s current selection's moving end to `point`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionUpdate {
    /// The terminal entity whose selection is extended.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative moving end: line `0` is the top row of the
    /// *displayed* viewport, not the active grid.
    pub point: GridPoint,
    /// Which half of the cell the moving end sits in.
    pub side: CellSide,
}

/// Clears any active local selection on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionClear {
    /// The terminal entity whose selection is cleared.
    #[event_target]
    pub entity: Entity,
}

/// Copies `entity`'s current selection to the clipboard.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionCopy {
    /// The terminal entity whose selection is copied.
    #[event_target]
    pub entity: Entity,
}

/// Triggers a `TerminalSelectionCopy` on the focused terminal, if any. Used by
/// the shortcut applier's `Shortcut::Copy` arm.
pub(crate) fn trigger_selection_copy(commands: &mut Commands, focused: Option<Entity>) {
    if let Some(entity) = focused {
        commands.trigger(TerminalSelectionCopy { entity });
    }
}

/// Registers the selection apply observers.
pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_selection_start)
            .add_observer(on_terminal_selection_update)
            .add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy)
            .add_observer(on_selection_text);
    }
}

/// Applies a `TerminalSelectionStart`: converts the viewport-relative point
/// to active-grid coordinates against the displayed frame's display offset —
/// the basis the click was hit-tested against — then requests the same start
/// on the underlying tty.
fn on_terminal_selection_start(
    ev: On<TerminalSelectionStart>,
    mut commands: Commands,
    terminals: Query<&TerminalGrid>,
) {
    let Ok(grid) = terminals.get(ev.entity) else {
        return;
    };
    commands.trigger(RequestTtySelectionStart {
        terminal: ev.entity,
        cell: to_grid_point(ev.point, DisplayOffset(grid.display_offset)),
        side: ev.side,
        kind: ev.ty,
    });
}

/// Applies a `TerminalSelectionUpdate`: same viewport-to-grid conversion as
/// start, then requests the same update on the underlying tty.
fn on_terminal_selection_update(
    ev: On<TerminalSelectionUpdate>,
    mut commands: Commands,
    terminals: Query<&TerminalGrid>,
) {
    let Ok(grid) = terminals.get(ev.entity) else {
        return;
    };
    commands.trigger(RequestTtySelectionUpdate {
        terminal: ev.entity,
        cell: to_grid_point(ev.point, DisplayOffset(grid.display_offset)),
        side: ev.side,
    });
}

/// Applies a `TerminalSelectionClear` by requesting the same clear on the
/// underlying tty.
fn on_terminal_selection_clear(ev: On<TerminalSelectionClear>, mut commands: Commands) {
    commands.trigger(RequestTtySelectionClear {
        terminal: ev.entity,
    });
}

/// Applies a `TerminalSelectionCopy`: asks the backend for the pane's
/// selected text. The answer arrives as `TtySelectionTextSignal`.
fn on_terminal_selection_copy(
    ev: On<TerminalSelectionCopy>,
    mut commands: Commands,
    terminals: Query<(), With<OrzmaTerminal>>,
) {
    if terminals.get(ev.entity).is_ok() {
        commands.trigger(RequestTtyCopySelection {
            terminal: ev.entity,
        });
    }
}

/// Writes an answered copy to the clipboard, skipping empty text so the
/// clipboard is never overwritten with nothing.
fn on_selection_text(ev: On<TtySelectionTextSignal>, mut commands: Commands) {
    if let Some(text) = ev.text.as_ref().filter(|t| !t.is_empty()) {
        commands.trigger(CopyAction { text: text.clone() });
    }
}

/// Converts a viewport-relative point (`mouse.rs`'s contract: line `0` is
/// the top of the displayed viewport) into the active-grid coordinates
/// `RequestTtySelectionStart`/`RequestTtySelectionUpdate` document their
/// `cell` field as expecting, delegating the projection to
/// `ViewportLine::to_grid`.
fn to_grid_point(viewport_point: GridPoint, offset: DisplayOffset) -> GridPoint {
    let viewport_line = u16::try_from(viewport_point.line.0)
        .expect("the selection events document a non-negative viewport line");
    GridPoint {
        line: ViewportLine(viewport_line).to_grid(offset),
        column: viewport_point.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::{GridColumn, GridLine};

    #[derive(Resource, Default)]
    struct SeenStarts(Vec<(Entity, GridPoint, CellSide, SelectionKind)>);
    #[derive(Resource, Default)]
    struct SeenUpdates(Vec<(Entity, GridPoint, CellSide)>);
    #[derive(Resource, Default)]
    struct SeenClears(Vec<Entity>);

    fn spawn_scrolled_grid(app: &mut App, display_offset: u32) -> Entity {
        app.world_mut()
            .spawn(TerminalGrid {
                display_offset,
                ..TerminalGrid::default()
            })
            .id()
    }

    /// Asserts that `TerminalSelectionStart` converts its viewport-relative
    /// point into active-grid coordinates by subtracting the displayed
    /// frame's display offset, before forwarding as `RequestTtySelectionStart`.
    ///
    /// Case: the user presses the mouse button on a cell while the viewport
    /// is scrolled back into history, so the clicked row's grid line differs
    /// from its on-screen row.
    #[test]
    fn selection_start_converts_viewport_point_to_grid_coordinates() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SeenStarts>()
            .add_observer(on_terminal_selection_start)
            .add_observer(
                |ev: On<RequestTtySelectionStart>, mut seen: ResMut<SeenStarts>| {
                    seen.0.push((ev.terminal, ev.cell, ev.side, ev.kind));
                },
            );
        let entity = spawn_scrolled_grid(&mut app, 4);
        let viewport_point = GridPoint {
            line: GridLine(1),
            column: GridColumn(2),
        };

        app.world_mut().trigger(TerminalSelectionStart {
            entity,
            point: viewport_point,
            side: CellSide::Left,
            ty: SelectionKind::Simple,
        });
        app.update();

        let expected_cell = GridPoint {
            line: GridLine(1 - 4),
            column: GridColumn(2),
        };
        assert_eq!(
            app.world().resource::<SeenStarts>().0,
            vec![(entity, expected_cell, CellSide::Left, SelectionKind::Simple)]
        );
    }

    /// Asserts that a selection start aimed at an entity without a terminal
    /// grid triggers nothing — there is no display offset to convert
    /// against.
    ///
    /// Case: a press event in flight while its target pane is torn down.
    #[test]
    fn selection_start_on_a_bare_entity_triggers_nothing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SeenStarts>()
            .add_observer(on_terminal_selection_start)
            .add_observer(
                |ev: On<RequestTtySelectionStart>, mut seen: ResMut<SeenStarts>| {
                    seen.0.push((ev.terminal, ev.cell, ev.side, ev.kind));
                },
            );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(TerminalSelectionStart {
            entity,
            point: GridPoint::default(),
            side: CellSide::Left,
            ty: SelectionKind::Simple,
        });
        app.update();

        assert!(app.world().resource::<SeenStarts>().0.is_empty());
    }

    /// Asserts that `TerminalSelectionUpdate` converts its viewport-relative
    /// point the same way as start.
    ///
    /// Case: the user drags the mouse to extend a selection while the
    /// viewport is scrolled back.
    #[test]
    fn selection_update_converts_viewport_point_to_grid_coordinates() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SeenUpdates>()
            .add_observer(on_terminal_selection_update)
            .add_observer(
                |ev: On<RequestTtySelectionUpdate>, mut seen: ResMut<SeenUpdates>| {
                    seen.0.push((ev.terminal, ev.cell, ev.side));
                },
            );
        let entity = spawn_scrolled_grid(&mut app, 2);
        let viewport_point = GridPoint {
            line: GridLine(0),
            column: GridColumn(5),
        };

        app.world_mut().trigger(TerminalSelectionUpdate {
            entity,
            point: viewport_point,
            side: CellSide::Right,
        });
        app.update();

        let expected_cell = GridPoint {
            line: GridLine(0 - 2),
            column: GridColumn(5),
        };
        assert_eq!(
            app.world().resource::<SeenUpdates>().0,
            vec![(entity, expected_cell, CellSide::Right)]
        );
    }

    /// Asserts that `TerminalSelectionClear` is forwarded as a
    /// `RequestTtySelectionClear` targeting the same entity.
    ///
    /// Case: the user clicks elsewhere to dismiss an existing selection.
    #[test]
    fn selection_clear_triggers_the_matching_request() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SeenClears>()
            .add_observer(on_terminal_selection_clear)
            .add_observer(
                |ev: On<RequestTtySelectionClear>, mut seen: ResMut<SeenClears>| {
                    seen.0.push(ev.terminal);
                },
            );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(TerminalSelectionClear { entity });
        app.update();

        assert_eq!(app.world().resource::<SeenClears>().0, vec![entity]);
    }

    /// Asserts that a copy on a terminal entity asks the backend for its
    /// selection, and that an answered text becomes a clipboard write
    /// while an empty answer does not.
    ///
    /// Case: the user presses Cmd+C once with a selection and once with
    /// nothing selected.
    #[test]
    fn copy_asks_the_backend_and_writes_only_non_empty_answers() {
        #[derive(Resource, Default)]
        struct Seen {
            requests: Vec<Entity>,
            copies: Vec<String>,
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SelectionPlugin)
            .init_resource::<Seen>()
            .add_observer(|ev: On<RequestTtyCopySelection>, mut s: ResMut<Seen>| {
                s.requests.push(ev.terminal)
            })
            .add_observer(|ev: On<CopyAction>, mut s: ResMut<Seen>| s.copies.push(ev.text.clone()));
        let terminal = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut()
            .trigger(TerminalSelectionCopy { entity: terminal });
        app.world_mut().trigger(TtySelectionTextSignal {
            text: Some("hello".into()),
        });
        app.world_mut().trigger(TtySelectionTextSignal {
            text: Some(String::new()),
        });
        app.world_mut()
            .trigger(TtySelectionTextSignal { text: None });
        app.update();
        let seen = app.world().resource::<Seen>();
        assert_eq!(seen.requests, vec![terminal]);
        assert_eq!(seen.copies, vec!["hello".to_string()]);
    }
}
