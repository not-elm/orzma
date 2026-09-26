//! The connections that completed their `hello`, and the pane each one
//! belongs to.

use crate::control_socket::ConnectionId;
use crate::host::PaneKey;
use std::collections::HashMap;

/// The pane each hello'd connection belongs to.
pub(crate) struct Connections<P> {
    panes: HashMap<ConnectionId, P>,
}

impl<P: PaneKey> Connections<P> {
    /// No connection.
    pub fn new() -> Self {
        Self {
            panes: HashMap::new(),
        }
    }

    /// Records that `connection` belongs to `pane`.
    pub fn insert(&mut self, connection: ConnectionId, pane: P) {
        self.panes.insert(connection, pane);
    }

    /// Forgets `connection`.
    pub fn remove(&mut self, connection: ConnectionId) {
        self.panes.remove(&connection);
    }

    /// The pane `connection` belongs to, or `None` when it never completed
    /// its `hello` or has closed.
    pub fn pane_of(&self, connection: ConnectionId) -> Option<P> {
        self.panes.get(&connection).copied()
    }
}
