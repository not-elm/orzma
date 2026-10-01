//! Tab management the host asks for, sent as the matching
//! `OrzmuxCommand`.

use crate::OrzmuxConnection;
use crate::tab::{PendingTabMove, PendingTabRename, RenameInFlight};
use bevy::prelude::*;
use orzmux::prelude::{CloseTarget, OrzmuxCommand, TabId, TabTarget};

/// A tab-management action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabAction {
    /// Display a tab.
    Select(TabTarget),
    /// Close a tab and every pane in it.
    Close(CloseTarget),
    /// Name a tab, or restore its automatic name with `None`. Recorded in
    /// `PendingTabRename` until the backend answers.
    Rename {
        /// The tab to name.
        tab: TabId,
        /// The new name.
        name: Option<String>,
    },
    /// Move a tab to a zero-based position; recorded in
    /// `PendingTabMove` until the backend answers.
    Move {
        /// The tab to move.
        tab: TabId,
        /// Its new position.
        index: u16,
    },
}

/// The host asks for a tab action.
#[derive(Event, Debug, Clone)]
pub struct RequestTabAction {
    /// The action to perform.
    pub action: TabAction,
}

pub(super) struct TabActionPlugin;

impl Plugin for TabActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_tab_action.run_if(resource_exists::<OrzmuxConnection>));
    }
}

/// Sends the action's command; a move's and a rename's sequences are
/// recorded as pending.
fn apply_tab_action(
    e: On<RequestTabAction>,
    mut pending_move: ResMut<PendingTabMove>,
    mut pending_rename: ResMut<PendingTabRename>,
    connection: Res<OrzmuxConnection>,
) {
    let command = match e.action.clone() {
        TabAction::Select(tab) => OrzmuxCommand::SelectTab { tab },
        TabAction::Close(tab) => OrzmuxCommand::CloseTab { tab },
        TabAction::Rename { tab, name } => OrzmuxCommand::RenameTab { tab, name },
        TabAction::Move { tab, index } => OrzmuxCommand::MoveTab { tab, index },
    };
    let seq = connection.0.send(command);
    match &e.action {
        TabAction::Move { .. } => pending_move.0 = Some(seq),
        TabAction::Rename { tab, name } => {
            pending_rename.0.insert(
                *tab,
                RenameInFlight {
                    seq,
                    name: name.clone(),
                },
            );
        }
        TabAction::Select(_) | TabAction::Close(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::app_with_connection;
    use orzmux::prelude::CommandSeq;

    /// Asserts that each tab action becomes its command, and that a move
    /// and a rename record their sequences as pending.
    ///
    /// Case: the user clicks a tab, closes another, renames one, and
    /// drops a dragged tab.
    #[test]
    fn tab_actions_become_commands_and_moves_and_renames_are_recorded() {
        let (mut app, commands) = app_with_connection(TabActionPlugin);
        for action in [
            TabAction::Select(TabTarget::Id(TabId(2))),
            TabAction::Close(CloseTarget::Active),
            TabAction::Rename {
                tab: TabId(1),
                name: Some("logs".into()),
            },
            TabAction::Move {
                tab: TabId(1),
                index: 1,
            },
        ] {
            app.world_mut().trigger(RequestTabAction { action });
        }
        app.update();
        let sent: Vec<(CommandSeq, OrzmuxCommand)> = commands.try_iter().collect();
        assert!(matches!(sent[0].1, OrzmuxCommand::SelectTab { .. }));
        assert!(matches!(sent[1].1, OrzmuxCommand::CloseTab { .. }));
        assert!(matches!(sent[2].1, OrzmuxCommand::RenameTab { .. }));
        assert!(matches!(sent[3].1, OrzmuxCommand::MoveTab { .. }));
        assert_eq!(app.world().resource::<PendingTabMove>().0, Some(sent[3].0));
        assert_eq!(
            app.world().resource::<PendingTabRename>().0.get(&TabId(1)),
            Some(&RenameInFlight {
                seq: sent[2].0,
                name: Some("logs".into()),
            })
        );
    }
}
