//! The workspace list the backend last reported, and the move the tab bar
//! waits on.

use bevy::prelude::*;
use orzmux::prelude::{CommandSeq, WorkspaceEntry, WorkspaceId};

/// The backend's latest workspace list, changed only when it differs.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct CurrentWorkspaces {
    /// Every workspace in display order.
    pub entries: Vec<WorkspaceEntry>,
    /// The displayed workspace; `None` before the first workspace opens
    /// and after the last one closes.
    pub active: Option<WorkspaceId>,
}

impl CurrentWorkspaces {
    /// The zero-based display position of `id`.
    pub fn position_of(&self, id: WorkspaceId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }
}

/// The sequence of the last `MoveWorkspace` the GUI sent whose answering
/// `Workspaces` has not arrived yet.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingWorkspaceMove(pub Option<CommandSeq>);
