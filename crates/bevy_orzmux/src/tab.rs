//! The tab list the backend last reported, and the move and rename the
//! tab bar waits on.

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
    /// The name `entry`'s tab shows: the one its rename in flight sets, or
    /// the one the backend reported when none is in flight. `None` stands
    /// for the automatic label.
    pub fn name_of<'a>(&'a self, entry: &'a TabEntry) -> Option<&'a str> {
        match self.0.get(&entry.id) {
            Some(rename) => rename.name.as_deref(),
            None => entry.name.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzmux::prelude::PaneId;

    fn entry(id: u32, name: Option<&str>) -> TabEntry {
        TabEntry {
            id: TabId(id),
            name: name.map(str::to_owned),
            active_pane: PaneId(id),
        }
    }

    /// Asserts that a rename in flight replaces only its own tab's
    /// reported name, and that one restoring the automatic label hides the
    /// reported name.
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
        assert_eq!(pending.name_of(&entry(2, None)), Some("logs"));
        assert_eq!(pending.name_of(&entry(1, Some("old"))), None);
        assert_eq!(pending.name_of(&entry(3, Some("kept"))), Some("kept"));
    }
}
