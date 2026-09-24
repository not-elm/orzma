//! Per-operation selection request events the host UI fires at a
//! terminal entity.
//!
//! TODO: apply the vi-cursor start and the kind change once vi mode
//! lands in the backend.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
pub use orzma_vt::prelude::{CellSide, GridPoint, SelectionKind};
use orzmux::prelude::OrzmuxCommand;

/// Fired by the host UI to anchor a new selection at the vi cursor
/// (vi-mode `v` / `V`). The backend has no vi mode, so applying it does
/// nothing.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionStartAtViCursor {
    #[event_target]
    pub terminal: Entity,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to switch selection granularity while keeping
/// the anchor (vi-mode `v` while `V` is active, and the reverse). The
/// backend has no vi mode, so applying it does nothing.
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
        app.add_observer(start_selection_at_vi_cursor)
            .add_observer(change_selection_kind)
            .add_observer(clear_selection.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn start_selection_at_vi_cursor(_e: On<RequestTtySelectionStartAtViCursor>) {}

fn change_selection_kind(_e: On<RequestTtySelectionKindChange>) {}

fn clear_selection(e: On<RequestTtySelectionClear>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::SelectionClear { pane });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzmux::prelude::PaneId;

    /// Asserts that a clear request becomes a `SelectionClear` command for
    /// the addressed pane, and one for a non-pane entity sends nothing.
    ///
    /// Case: the user copies a selection with Ctrl+C, which clears it,
    /// while a stray request targets the separator entity.
    #[test]
    fn a_clear_request_becomes_a_selection_clear_for_the_pane() {
        let (mut app, commands) = app_with_connection(SelectionPlugin);
        let pane = spawn_pane(&mut app, PaneId(3));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(RequestTtySelectionClear { terminal: pane });
        app.world_mut()
            .trigger(RequestTtySelectionClear { terminal: stray });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::SelectionClear { pane: PaneId(3) }
        ));
    }
}
