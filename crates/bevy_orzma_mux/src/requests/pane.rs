//! `RequestPaneAction`: pane management the host asks for (directional
//! selection, kill, click-to-focus), sent as the matching `MuxCommand`.

use crate::registry::PaneRegistry;
use crate::{MuxConnection, MuxPane};
use bevy::prelude::*;
use orzma_mux::prelude::{MuxCommand, PaneDirection, PaneTarget};

/// A pane-management action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneAction {
    /// Move the active pane to its neighbour.
    SelectDirection(PaneDirection),
    /// Kill the active pane.
    Kill,
    /// Make the pane behind `entity` active (a click).
    Select(Entity),
}

/// The host asks for a pane action.
#[derive(Event, Debug, Clone, Copy)]
pub struct RequestPaneAction {
    /// The action to perform.
    pub action: PaneAction,
}

pub(super) struct PaneActionPlugin;

impl Plugin for PaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_pane_action);
    }
}

fn apply_pane_action(
    e: On<RequestPaneAction>,
    mut registry: ResMut<PaneRegistry>,
    connection: Res<MuxConnection>,
    panes: Query<&MuxPane>,
) {
    match e.action {
        PaneAction::SelectDirection(direction) => {
            let seq = connection
                .0
                .send(MuxCommand::SelectPaneDirection { direction });
            registry.last_select = Some(seq);
        }
        PaneAction::Kill => {
            connection.0.send(MuxCommand::KillPane {
                pane: PaneTarget::Active,
            });
        }
        PaneAction::Select(entity) => {
            if let Ok(pane) = panes.get(entity) {
                let seq = connection.0.send(MuxCommand::SelectPane { pane: pane.0 });
                registry.last_select = Some(seq);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_mux::prelude::{CommandSeq, PaneId};

    /// Asserts that every pane action becomes its command and that
    /// selections record their sequence for stale-layout filtering.
    ///
    /// Case: the user presses select-right, kill-pane, then clicks a pane.
    #[test]
    fn pane_actions_become_commands_and_selections_record_their_seq() {
        let (mut app, commands) = app_with_connection(PaneActionPlugin);
        let pane = spawn_pane(&mut app, PaneId(5));
        app.world_mut().trigger(RequestPaneAction {
            action: PaneAction::SelectDirection(PaneDirection::Right),
        });
        app.world_mut().trigger(RequestPaneAction {
            action: PaneAction::Kill,
        });
        app.world_mut().trigger(RequestPaneAction {
            action: PaneAction::Select(pane),
        });
        let sent = sent(&commands);
        assert!(matches!(
            sent[0],
            MuxCommand::SelectPaneDirection {
                direction: PaneDirection::Right
            }
        ));
        assert!(matches!(
            sent[1],
            MuxCommand::KillPane {
                pane: PaneTarget::Active
            }
        ));
        assert!(matches!(
            sent[2],
            MuxCommand::SelectPane { pane: PaneId(5) }
        ));
        assert_eq!(
            app.world().resource::<PaneRegistry>().last_select,
            Some(CommandSeq(3))
        );
    }
}
