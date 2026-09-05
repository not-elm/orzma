//! Per-operation selection request events the host UI fires at a
//! terminal entity.
//!
//! The payload vocabulary ([`SelectionKind`], [`CellSide`],
//! [`GridPoint`]) is owned by the VT layer; this module re-exports
//! it so the requests and their payload types travel together — each
//! request carries exactly what the backend applies.
//!
//! The start, update, and clear observers send the matching
//! `MuxCommand`; the vi-cursor start and the kind change stay stubs
//! until vi mode lands in the backend.

use crate::{MuxConnection, MuxPane};
use bevy::prelude::*;
use orzma_mux::prelude::MuxCommand;
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

fn start_selection(
    e: On<RequestTtySelectionStart>,
    connection: Res<MuxConnection>,
    panes: Query<&MuxPane>,
) {
    if let Ok(pane) = panes.get(e.terminal) {
        connection.0.send(MuxCommand::SelectionStart {
            pane: pane.0,
            cell: e.cell,
            side: e.side,
            kind: e.kind,
        });
    }
}

fn start_selection_at_vi_cursor(_e: On<RequestTtySelectionStartAtViCursor>) {}

fn update_selection(
    e: On<RequestTtySelectionUpdate>,
    connection: Res<MuxConnection>,
    panes: Query<&MuxPane>,
) {
    if let Ok(pane) = panes.get(e.terminal) {
        connection.0.send(MuxCommand::SelectionUpdate {
            pane: pane.0,
            cell: e.cell,
            side: e.side,
        });
    }
}

fn change_selection_kind(_e: On<RequestTtySelectionKindChange>) {}

fn clear_selection(
    e: On<RequestTtySelectionClear>,
    connection: Res<MuxConnection>,
    panes: Query<&MuxPane>,
) {
    if let Ok(pane) = panes.get(e.terminal) {
        connection
            .0
            .send(MuxCommand::SelectionClear { pane: pane.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_mux::prelude::PaneId;
    use orzma_vt::prelude::{GridColumn, GridLine};

    fn cell(line: i32, column: u16) -> GridPoint {
        GridPoint {
            line: GridLine(line),
            column: GridColumn(column),
        }
    }

    /// Asserts that start, update, and clear each become their matching
    /// `MuxCommand` for the addressed pane, and a non-pane entity sends
    /// nothing.
    ///
    /// Case: the user presses on a cell, drags to another, then clicks
    /// elsewhere to clear it, while a stray request targets the
    /// separator entity.
    #[test]
    fn selection_requests_become_their_matching_commands_for_the_pane() {
        let (mut app, commands) = app_with_connection(SelectionPlugin);
        let pane = spawn_pane(&mut app, PaneId(3));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtySelectionStart {
            terminal: pane,
            cell: cell(0, 1),
            side: CellSide::Left,
            kind: SelectionKind::Simple,
        });
        app.world_mut().trigger(RequestTtySelectionUpdate {
            terminal: pane,
            cell: cell(0, 2),
            side: CellSide::Right,
        });
        app.world_mut()
            .trigger(RequestTtySelectionClear { terminal: pane });
        app.world_mut().trigger(RequestTtySelectionUpdate {
            terminal: stray,
            cell: cell(0, 0),
            side: CellSide::Left,
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 3);
        assert!(matches!(
            sent[0],
            MuxCommand::SelectionStart {
                pane: PaneId(3),
                side: CellSide::Left,
                kind: SelectionKind::Simple,
                ..
            }
        ));
        assert!(matches!(
            sent[1],
            MuxCommand::SelectionUpdate {
                pane: PaneId(3),
                side: CellSide::Right,
                ..
            }
        ));
        assert!(matches!(
            sent[2],
            MuxCommand::SelectionClear { pane: PaneId(3) }
        ));
    }
}
