//! The tab list the backend last reported, and the move and rename the tab bar waits on.

use bevy::prelude::*;
use orzmux::prelude::{CommandSeq, TabEntry, TabId};
use std::collections::HashMap;

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

/// A `RenameTab` the GUI sent whose answering `Tabs` has not arrived yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameInFlight {
    /// The sequence the rename was sent with.
    pub seq: CommandSeq,
    /// The name it sets; `None` restores the automatic label.
    pub name: Option<String>,
}

/// The renames the tab bar shows before the backend answers them, one per
/// tab: the latest rename of each tab.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct PendingTabRename(pub HashMap<TabId, RenameInFlight>);

impl PendingTabRename {
    /// The name a rename in flight gives `tab`: `Some(name)` while one is in
    /// flight for it, where `name` is `None` when the rename restores the
    /// automatic label, and `None` otherwise.
    pub fn name_for(&self, tab: TabId) -> Option<Option<&str>> {
        self.0.get(&tab).map(|rename| rename.name.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that each rename in flight names only its own tab, and that
    /// a rename restoring the automatic label reads as `Some(None)`.
    ///
    /// Case: the user renames the second tab, then clears the name of the
    /// first, while both answers are on their way.
    #[test]
    fn a_rename_in_flight_names_only_its_tab() {
        let mut pending = PendingTabRename::default();
        pending.0.insert(
            TabId(2),
            RenameInFlight {
                seq: CommandSeq(3),
                name: Some("logs".into()),
            },
        );
        pending.0.insert(
            TabId(1),
            RenameInFlight {
                seq: CommandSeq(4),
                name: None,
            },
        );
        assert_eq!(pending.name_for(TabId(2)), Some(Some("logs")));
        assert_eq!(pending.name_for(TabId(1)), Some(None));
        assert_eq!(pending.name_for(TabId(3)), None);
    }
}
