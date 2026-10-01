//! The tab list the backend last reported, and the move the tab bar
//! waits on.

use bevy::prelude::*;
use orzmux::prelude::{CommandSeq, TabEntry, TabId};

/// The backend's latest tab list, changed only when it differs.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct CurrentTabs {
    /// Every tab in display order.
    pub entries: Vec<TabEntry>,
    /// The displayed tab; `None` before the first tab opens
    /// and after the last one closes.
    pub active: Option<TabId>,
}

impl CurrentTabs {
    /// The zero-based display position of `id`.
    pub fn position_of(&self, id: TabId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }
}

/// The sequence of the last `MoveTab` the GUI sent whose answering
/// `Tabs` has not arrived yet.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTabMove(pub Option<CommandSeq>);
