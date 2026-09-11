//! The bridge's correlation tables: which entity mirrors which backend
//! pane, which spawn requests are in flight, and the focus state the GUI
//! last accepted.

use bevy::prelude::*;
use orzmux::prelude::{CommandSeq, PaneId, RequestId};
use std::collections::HashMap;

/// Correlation state between backend ids and entities.
#[derive(Resource, Default, Debug)]
pub struct PaneRegistry {
    /// Lookup cache, filled on `PaneOpened` and emptied on `PaneClosed`.
    /// An empty map does not mean the session ended; it is also empty
    /// before the first pane opens.
    pub panes: HashMap<PaneId, Entity>,
    /// Entities pre-spawned for a `NewPane` whose answer is pending.
    pub pending_spawns: HashMap<RequestId, Entity>,
    /// The sequence of the last `SelectPane` the GUI sent.
    pub last_select: Option<CommandSeq>,
    /// The active pane the GUI applied: accepted from a `Layout`, or
    /// taken optimistically from a click while its `SelectPane` is in
    /// flight.
    pub applied_active: Option<PaneId>,
}

impl PaneRegistry {
    /// The entity mirroring `pane`, if it is open.
    pub fn entity_of(&self, pane: PaneId) -> Option<Entity> {
        self.panes.get(&pane).copied()
    }
}
