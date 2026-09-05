//! Per-operation selection request events the host UI fires at a
//! terminal entity.
//!
//! The payload vocabulary ([`SelectionKind`], [`CellSide`],
//! [`GridPoint`]) is owned by the VT layer; this module re-exports
//! it so the requests and their payload types travel together — each
//! request carries exactly what the VT applies.
//!
//! The start, update, and clear observers route to the targeted
//! entity's handle; the vi-cursor start and the kind change stay stubs
//! until vi mode lands in the VT.

use crate::OrzmaTtyHandle;
use bevy::prelude::*;
pub use orzma_vt::prelude::{CellSide, GridPoint, SelectionKind};

/// Fired by the host UI to anchor a new selection at an explicit
/// grid cell (mouse press).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionStart {
    #[event_target]
    pub terminal: Entity,
    /// Grid cell the press landed on; the host UI resolves the
    /// clicked viewport cell against the displayed frame's display
    /// offset before firing.
    pub cell: GridPoint,
    /// Which half of the cell the anchor sits in.
    pub side: CellSide,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to anchor a new selection at the vi cursor
/// (vi-mode `v` / `V`), whose position only the VT knows.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionStartAtViCursor {
    #[event_target]
    pub terminal: Entity,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to move the moving end of the active selection
/// (mouse drag).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionUpdate {
    #[event_target]
    pub terminal: Entity,
    /// Grid cell the moving end is dragged to. May reach into
    /// scrollback history (a negative line) when the drag leaves the
    /// viewport.
    pub cell: GridPoint,
    /// Which half of the cell the moving end sits in.
    pub side: CellSide,
}

/// Fired by the host UI to switch selection granularity while keeping
/// the anchor (vi-mode `v` while `V` is active, and the reverse).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionKindChange {
    #[event_target]
    pub terminal: Entity,
    /// The granularity to switch to.
    pub kind: SelectionKind,
}

/// Fired by the host UI to drop any active selection.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionClear {
    #[event_target]
    pub terminal: Entity,
}

pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(start_selection)
            .add_observer(start_selection_at_vi_cursor)
            .add_observer(update_selection)
            .add_observer(change_selection_kind)
            .add_observer(clear_selection);
    }
}

fn start_selection(e: On<RequestTtySelectionStart>, mut terms: Query<&mut OrzmaTtyHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        tty.start_selection(e.cell, e.side, e.kind);
    }
}

fn start_selection_at_vi_cursor(_e: On<RequestTtySelectionStartAtViCursor>) {}

fn update_selection(e: On<RequestTtySelectionUpdate>, mut terms: Query<&mut OrzmaTtyHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        tty.extend_selection(e.cell, e.side);
    }
}

fn change_selection_kind(_e: On<RequestTtySelectionKindChange>) {}

fn clear_selection(e: On<RequestTtySelectionClear>, mut terms: Query<&mut OrzmaTtyHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        tty.clear_selection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::{GridColumn, GridLine, Vt};

    fn app_with_terminal() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(SelectionPlugin);
        let (mut handle, _) = OrzmaTtyHandle::detached(4, 3);
        handle.feed_bytes(b"abcd\r\nefgh\r\nijkl");
        let terminal = app.world_mut().spawn(handle).id();
        (app, terminal)
    }

    fn cell(line: i32, column: u16) -> GridPoint {
        GridPoint {
            line: GridLine(line),
            column: GridColumn(column),
        }
    }

    fn selection_text(app: &App, terminal: Entity) -> Option<String> {
        app.world()
            .get::<OrzmaTtyHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt()
            .selection_text()
    }

    /// Asserts that a start followed by an update reaches the VT as one
    /// selection whose text the handle can read back.
    ///
    /// Case: the user presses on a cell and drags across two more.
    #[test]
    fn start_and_update_reach_the_vt() {
        let (mut app, terminal) = app_with_terminal();
        app.world_mut().trigger(RequestTtySelectionStart {
            terminal,
            cell: cell(0, 1),
            side: CellSide::Left,
            kind: SelectionKind::Simple,
        });
        app.world_mut().trigger(RequestTtySelectionUpdate {
            terminal,
            cell: cell(0, 2),
            side: CellSide::Right,
        });
        assert_eq!(selection_text(&app, terminal).as_deref(), Some("bc"));
    }

    /// Asserts that a clear request drops the selection the VT holds.
    ///
    /// Case: the user clicks elsewhere after selecting a row.
    #[test]
    fn clear_reaches_the_vt() {
        let (mut app, terminal) = app_with_terminal();
        app.world_mut().trigger(RequestTtySelectionStart {
            terminal,
            cell: cell(0, 0),
            side: CellSide::Left,
            kind: SelectionKind::Lines,
        });
        assert!(selection_text(&app, terminal).is_some());
        app.world_mut()
            .trigger(RequestTtySelectionClear { terminal });
        assert_eq!(selection_text(&app, terminal), None);
    }

    /// Asserts that a request aimed at an entity without a handle is
    /// ignored rather than panicking.
    ///
    /// Case: a drag update is in flight while its pane is torn down.
    #[test]
    fn a_request_on_a_bare_entity_is_ignored() {
        let mut app = App::new();
        app.add_plugins(SelectionPlugin);
        let terminal = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtySelectionUpdate {
            terminal,
            cell: cell(0, 0),
            side: CellSide::Left,
        });
    }
}
