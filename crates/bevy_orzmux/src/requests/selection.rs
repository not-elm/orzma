//! Per-operation selection request events the host UI fires at a
//! terminal entity.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
pub use orzma_vt::prelude::{CellSide, GridPoint, SelectionKind};
use orzmux::prelude::OrzmuxCommand;

/// Fired by the host UI to start, re-kind, or clear a vi-mode selection at
/// the vi cursor (vi-mode `v` / `V`); the terminal resolves the toggle
/// against its own selection.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyViSelectionToggle {
    #[event_target]
    pub terminal: Entity,
    /// The selection granularity the toggle names.
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
        app.add_observer(toggle_vi_selection.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(clear_selection.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn toggle_vi_selection(e: On<RequestTtyViSelectionToggle>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::ViSelectionToggle {
        pane,
        kind: e.kind,
    });
}

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

    /// Asserts that a vi selection toggle becomes a `ViSelectionToggle`
    /// command for the addressed pane.
    ///
    /// Case: the user presses `V` in vi mode on a pane.
    #[test]
    fn a_toggle_request_becomes_a_vi_selection_toggle_for_the_pane() {
        let (mut app, commands) = app_with_connection(SelectionPlugin);
        let pane = spawn_pane(&mut app, PaneId(6));
        app.world_mut().trigger(RequestTtyViSelectionToggle {
            terminal: pane,
            kind: SelectionKind::Lines,
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::ViSelectionToggle {
                pane: PaneId(6),
                kind: SelectionKind::Lines
            }
        ));
    }
}
