//! Workspace management the host asks for, sent as the matching
//! `OrzmuxCommand`.

use crate::OrzmuxConnection;
use crate::workspace::PendingWorkspaceMove;
use bevy::prelude::*;
use orzmux::prelude::{CloseTarget, OrzmuxCommand, WorkspaceId, WorkspaceTarget};

/// A workspace-management action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Display a workspace.
    Select(WorkspaceTarget),
    /// Close a workspace and every pane in it.
    Close(CloseTarget),
    /// Name a workspace, or restore its automatic name with `None`.
    Rename {
        /// The workspace to name.
        workspace: WorkspaceId,
        /// The new name.
        name: Option<String>,
    },
    /// Move a workspace to a zero-based position; recorded in
    /// `PendingWorkspaceMove` until the backend answers.
    Move {
        /// The workspace to move.
        workspace: WorkspaceId,
        /// Its new position.
        index: u16,
    },
}

/// The host asks for a workspace action.
#[derive(Event, Debug, Clone)]
pub struct RequestWorkspaceAction {
    /// The action to perform.
    pub action: WorkspaceAction,
}

pub(super) struct WorkspaceActionPlugin;

impl Plugin for WorkspaceActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_workspace_action.run_if(resource_exists::<OrzmuxConnection>));
    }
}

/// Sends the action's command; a move's sequence is recorded as pending.
fn apply_workspace_action(
    e: On<RequestWorkspaceAction>,
    mut pending: ResMut<PendingWorkspaceMove>,
    connection: Res<OrzmuxConnection>,
) {
    let command = match e.action.clone() {
        WorkspaceAction::Select(workspace) => OrzmuxCommand::SelectWorkspace { workspace },
        WorkspaceAction::Close(workspace) => OrzmuxCommand::CloseWorkspace { workspace },
        WorkspaceAction::Rename { workspace, name } => {
            OrzmuxCommand::RenameWorkspace { workspace, name }
        }
        WorkspaceAction::Move { workspace, index } => {
            OrzmuxCommand::MoveWorkspace { workspace, index }
        }
    };
    let seq = connection.0.send(command);
    if matches!(e.action, WorkspaceAction::Move { .. }) {
        pending.0 = Some(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::app_with_connection;
    use orzmux::prelude::CommandSeq;

    /// Asserts that each workspace action becomes its command and that a
    /// move records its sequence as pending.
    ///
    /// Case: the user clicks a tab, closes another, renames one, and
    /// drops a dragged tab.
    #[test]
    fn workspace_actions_become_commands_and_moves_are_recorded() {
        let (mut app, commands) = app_with_connection(WorkspaceActionPlugin);
        for action in [
            WorkspaceAction::Select(WorkspaceTarget::Id(WorkspaceId(2))),
            WorkspaceAction::Close(CloseTarget::Active),
            WorkspaceAction::Rename {
                workspace: WorkspaceId(1),
                name: Some("logs".into()),
            },
            WorkspaceAction::Move {
                workspace: WorkspaceId(1),
                index: 1,
            },
        ] {
            app.world_mut().trigger(RequestWorkspaceAction { action });
        }
        app.update();
        let sent: Vec<(CommandSeq, OrzmuxCommand)> = commands.try_iter().collect();
        assert!(matches!(sent[0].1, OrzmuxCommand::SelectWorkspace { .. }));
        assert!(matches!(sent[1].1, OrzmuxCommand::CloseWorkspace { .. }));
        assert!(matches!(sent[2].1, OrzmuxCommand::RenameWorkspace { .. }));
        assert!(matches!(sent[3].1, OrzmuxCommand::MoveWorkspace { .. }));
        assert_eq!(
            app.world().resource::<PendingWorkspaceMove>().0,
            Some(sent[3].0)
        );
    }
}
