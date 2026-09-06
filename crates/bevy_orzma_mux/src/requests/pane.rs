//! `RequestPaneAction`: pane management the host asks for (directional
//! selection, kill, click-to-focus), sent as the matching `MuxCommand`.

use crate::layout::{CurrentLayout, MuxActivePaneChanged};
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
    /// Make the pane behind `entity` active (a click). Applied
    /// optimistically: the GUI treats it as the active pane at once and
    /// the confirming `Layout` reconciles.
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

/// Sends the action's command. A `Select` of the pane the backend has
/// already confirmed active sends nothing; any other `Select` also
/// applies the pane as active right away and reports the change, so the
/// frame's keys already go to the clicked pane.
fn apply_pane_action(
    e: On<RequestPaneAction>,
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    connection: Res<MuxConnection>,
    current: Res<CurrentLayout>,
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
            let Ok(pane) = panes.get(entity) else {
                return;
            };
            let already_applied = registry.applied_active == Some(pane.0);
            let confirmed = registry
                .last_select
                .is_none_or(|sent| sent <= current.0.seq);
            if already_applied && confirmed {
                return;
            }
            let seq = connection.0.send(MuxCommand::SelectPane { pane: pane.0 });
            registry.last_select = Some(seq);
            if !already_applied {
                let previous = registry
                    .applied_active
                    .and_then(|active| registry.entity_of(active));
                registry.applied_active = Some(pane.0);
                commands.trigger(MuxActivePaneChanged {
                    previous,
                    current: Some(entity),
                });
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

    /// Asserts that a `Select` applies the pane as active at once and
    /// reports the change from the previously applied pane, and that a
    /// `Select` of the confirmed active pane sends nothing.
    ///
    /// Case: the user clicks an inactive pane, then clicks inside the
    /// pane that is already active to place a selection.
    #[test]
    fn a_select_applies_the_active_optimistically_and_a_confirmed_one_is_a_no_op() {
        #[derive(Resource, Default)]
        struct Changes(Vec<(Option<Entity>, Option<Entity>)>);

        let (mut app, commands) = app_with_connection(PaneActionPlugin);
        app.init_resource::<Changes>().add_observer(
            |ev: On<MuxActivePaneChanged>, mut changes: ResMut<Changes>| {
                changes.0.push((ev.previous, ev.current))
            },
        );
        let a = spawn_pane(&mut app, PaneId(1));
        let b = spawn_pane(&mut app, PaneId(2));
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .applied_active = Some(PaneId(1));

        app.world_mut().trigger(RequestPaneAction {
            action: PaneAction::Select(b),
        });
        app.update();
        assert_eq!(
            app.world().resource::<Changes>().0,
            vec![(Some(a), Some(b))]
        );
        assert_eq!(
            app.world().resource::<PaneRegistry>().applied_active,
            Some(PaneId(2))
        );
        assert!(matches!(
            sent(&commands).as_slice(),
            [MuxCommand::SelectPane { pane: PaneId(2) }]
        ));

        app.world_mut().resource_mut::<CurrentLayout>().0.seq = CommandSeq(1);
        app.world_mut().trigger(RequestPaneAction {
            action: PaneAction::Select(b),
        });
        app.update();
        assert_eq!(app.world().resource::<Changes>().0.len(), 1);
        assert!(sent(&commands).is_empty());
    }
}
