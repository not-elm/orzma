//! The `NonSend` resource that owns the in-world `tmux -CC` protocol client.

use bevy::ecs::entity::Entity;
use std::cell::RefCell;
use std::rc::Rc;
use tmux_control::{ClientEvent, CommandId, ProtocolClient, TmuxCommand, TmuxResult};

/// Owns the in-world `tmux -CC` connection, if any.
///
/// The connection is driven on the Bevy schedule: captured bytes from the
/// adopted gateway terminal's PTY are fed in with [`feed`](Self::feed), and the
/// protocol's outgoing bytes are drained with
/// [`take_outgoing`](Self::take_outgoing) and written back to that PTY.
///
/// NOTE: this holds `Rc<RefCell<ProtocolClient>>` and is therefore main-thread
/// only; it is inserted as a Bevy **`NonSend`** resource
/// (`app.insert_non_send_resource(TmuxConnection::default())`) and is
/// intentionally NOT a `Resource`.
#[derive(Default)]
pub struct TmuxConnection {
    adopted: Option<Adopted>,
}

struct Adopted {
    protocol: Rc<RefCell<ProtocolClient>>,
    gateway: Entity,
    client_name: Option<String>,
    per_window_refresh: Option<bool>,
}

impl TmuxConnection {
    /// Installs a fresh [`ProtocolClient`] driven over `gateway`'s adopted PTY,
    /// replacing any prior connection.
    ///
    /// Pre-registers the single reply block the adopted `tmux -CC` stream emits
    /// on entry (via `register_external_pending`) and returns its [`CommandId`]
    /// so the caller can record it in its pending-reply state.
    pub fn adopt(&mut self, gateway: Entity) -> CommandId {
        let mut protocol = ProtocolClient::new();
        let pending = protocol.register_external_pending();
        self.adopted = Some(Adopted {
            protocol: Rc::new(RefCell::new(protocol)),
            gateway,
            client_name: None,
            per_window_refresh: None,
        });
        pending
    }

    /// Returns a cheap send handle for the live connection, or `None` when
    /// disconnected.
    pub fn handle(&self) -> Option<AdoptedHandle> {
        self.adopted.as_ref().map(|a| AdoptedHandle {
            protocol: Rc::clone(&a.protocol),
        })
    }

    /// Returns the adopted gateway terminal entity, or `None` when disconnected.
    pub fn gateway(&self) -> Option<Entity> {
        self.adopted.as_ref().map(|a| a.gateway)
    }

    /// Returns whether a connection is currently installed.
    pub fn is_connected(&self) -> bool {
        self.adopted.is_some()
    }

    /// Feeds a raw byte chunk (from the gateway PTY) through the protocol,
    /// returning the [`ClientEvent`]s it produced. Returns an empty vec when
    /// disconnected.
    pub fn feed(&self, bytes: &[u8]) -> TmuxResult<Vec<ClientEvent>> {
        match &self.adopted {
            Some(a) => a.protocol.borrow_mut().feed(bytes),
            None => Ok(Vec::new()),
        }
    }

    /// Drains the protocol's outgoing buffer for the caller to write back to the
    /// gateway PTY. Returns an empty vec when disconnected.
    pub fn take_outgoing(&self) -> Vec<u8> {
        match &self.adopted {
            Some(a) => a.protocol.borrow_mut().take_outgoing(),
            None => Vec::new(),
        }
    }

    /// Tears down the live connection, returning the gateway entity that was
    /// adopted (so the caller can despawn it), or `None` when already
    /// disconnected.
    pub fn close(&mut self) -> Option<Entity> {
        self.adopted.take().map(|a| a.gateway)
    }

    /// Returns the control client's name as reported by tmux, or `None` if the
    /// name query has not yet completed (or the connection is absent).
    pub fn client_name(&self) -> Option<&str> {
        self.adopted.as_ref().and_then(|a| a.client_name.as_deref())
    }

    /// Returns whether the attached tmux supports per-window `refresh-client`,
    /// or `None` if the version query has not completed yet (or the connection
    /// is absent).
    pub fn supports_per_window_refresh(&self) -> Option<bool> {
        self.adopted.as_ref().and_then(|a| a.per_window_refresh)
    }

    /// Caches the control client name returned by the `display-message` query.
    pub(crate) fn set_client_name(&mut self, name: String) {
        if let Some(a) = self.adopted.as_mut() {
            a.client_name = Some(name);
        }
    }

    /// Caches the per-window `refresh-client` capability derived from the tmux
    /// version reply.
    pub(crate) fn set_per_window_refresh(&mut self, supported: bool) {
        if let Some(a) = self.adopted.as_mut() {
            a.per_window_refresh = Some(supported);
        }
    }
}

/// A cheap, cloneable send handle for the in-world tmux connection.
///
/// Obtained from [`TmuxConnection::handle`]; sending borrows the shared
/// [`ProtocolClient`] only for the duration of the call, queueing the command's
/// bytes into the protocol's outgoing buffer (flushed to the PTY by the drive's
/// outbound system).
pub struct AdoptedHandle {
    protocol: Rc<RefCell<ProtocolClient>>,
}

impl AdoptedHandle {
    /// Encodes and queues `cmd`, returning its [`CommandId`].
    pub fn send(&self, cmd: impl TmuxCommand) -> TmuxResult<CommandId> {
        self.protocol.borrow_mut().send(&cmd.into_raw_command())
    }

    /// Queues an already-rendered command string, returning its [`CommandId`].
    pub fn send_raw(&self, cmd: &str) -> TmuxResult<CommandId> {
        self.protocol.borrow_mut().send(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_then_send_and_feed_roundtrip() {
        let mut conn = TmuxConnection::default();
        let gateway = Entity::from_raw_u32(7).expect("entity id");
        let _pending = conn.adopt(gateway);
        assert_eq!(conn.gateway(), Some(gateway));
        assert!(conn.is_connected());

        let h = conn.handle().expect("handle");
        let _id = h.send_raw("list-windows").expect("send");
        let out = conn.take_outgoing();
        assert_eq!(out, b"list-windows\n");

        let events = conn
            .feed(b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n")
            .expect("feed");
        assert!(matches!(
            events.as_slice(),
            [ClientEvent::CommandComplete { .. }]
        ));

        assert_eq!(conn.close(), Some(gateway));
        assert!(!conn.is_connected());
    }
}
