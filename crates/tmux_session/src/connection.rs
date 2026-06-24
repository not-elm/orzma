//! The `TmuxClient` component that owns the in-world `tmux -CC` protocol client.

use crate::enumerate::EnumerationState;
use bevy::ecs::component::Component;
use bevy::ecs::system::Single;
use tmux_control::{ClientEvent, CommandId, ProtocolClient, TmuxCommand, TmuxResult};

/// A `tmux -CC` control client owned as a component on the gateway entity.
///
/// Holds the sans-IO [`ProtocolClient`] directly (no `Rc`/`RefCell`): the
/// component is accessed through exclusive `&mut` query access, so interior
/// mutability is unnecessary. Inserted on adoption and removed when the gateway
/// entity is despawned on teardown. Requires [`EnumerationState`] so a gateway
/// always carries its in-flight reply correlation alongside the client.
#[derive(Component, Debug, Default)]
#[require(EnumerationState)]
pub struct TmuxClient {
    protocol: ProtocolClient,
    client_name: Option<String>,
    per_window_refresh: Option<bool>,
}

impl TmuxClient {
    /// Returns a client for a freshly adopted `tmux -CC` stream.
    ///
    /// Pre-registers the single reply block the adopted stream emits on entry
    /// (its DCS introducer is glued to the first `%begin`) so the in-world drive
    /// correlates it instead of dropping it as unsolicited.
    pub fn new_adopted() -> Self {
        let mut protocol = ProtocolClient::new();
        let _entry = protocol.register_external_pending();
        Self {
            protocol,
            client_name: None,
            per_window_refresh: None,
        }
    }

    /// Feeds a raw byte chunk (from the gateway PTY) through the protocol,
    /// returning the [`ClientEvent`]s it produced.
    pub fn feed(&mut self, bytes: &[u8]) -> TmuxResult<Vec<ClientEvent>> {
        self.protocol.feed(bytes)
    }

    /// Drains the protocol's outgoing buffer for the caller to write back to the
    /// gateway PTY.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        self.protocol.take_outgoing()
    }

    /// Encodes and queues `cmd`, returning its [`CommandId`].
    pub fn send(&mut self, cmd: impl TmuxCommand) -> TmuxResult<CommandId> {
        self.protocol.send(&cmd.into_raw_command())
    }

    /// Queues an already-rendered command string, returning its [`CommandId`].
    pub fn send_raw(&mut self, cmd: &str) -> TmuxResult<CommandId> {
        self.protocol.send(cmd)
    }

    /// Returns the control client's name as reported by tmux, or `None` if the
    /// name query has not yet completed.
    pub fn client_name(&self) -> Option<&str> {
        self.client_name.as_deref()
    }

    /// Returns whether the attached tmux supports per-window `refresh-client`,
    /// or `None` if the version query has not completed yet.
    pub fn supports_per_window_refresh(&self) -> Option<bool> {
        self.per_window_refresh
    }

    /// Caches the control client name returned by the `display-message` query.
    pub(crate) fn set_client_name(&mut self, name: String) {
        self.client_name = Some(name);
    }

    /// Caches the per-window `refresh-client` capability from the version reply.
    pub(crate) fn set_per_window_refresh(&mut self, supported: bool) {
        self.per_window_refresh = Some(supported);
    }
}

/// Marks a [`TmuxClient`] entity that has received its first protocol event.
///
/// Inserted on the attach edge (the first `TmuxEventBatch` protocol event after
/// adoption). `With<TmuxAttached>` is the "attached" guard; `Added<TmuxAttached>`
/// is the attach edge for one-shot work.
#[derive(Component, Debug, Default)]
pub struct TmuxAttached;

/// A `Single` query for mutable access to the live [`TmuxClient`].
///
/// Auto-skips the system when there is not exactly one client. A type alias (not
/// a custom `SystemParam`) so the future multi-session migration to `Query` has
/// one named anchor. When used as a system param, `&'static mut TmuxClient`
/// resolves to `Mut<'w, TmuxClient>`.
pub type TmuxClientMut<'w, 's> = Single<'w, 's, &'static mut TmuxClient>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_client_send_and_feed_roundtrip() {
        let mut client = TmuxClient::new_adopted();
        let _id = client.send_raw("list-windows").expect("send");
        assert_eq!(client.take_outgoing(), b"list-windows\n");

        let events = client
            .feed(b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n")
            .expect("feed");
        assert!(matches!(
            events.as_slice(),
            [ClientEvent::CommandComplete { .. }]
        ));
    }

    #[test]
    fn tmux_client_caches_default_to_none() {
        let mut client = TmuxClient::new_adopted();
        assert_eq!(client.client_name(), None);
        assert_eq!(client.supports_per_window_refresh(), None);
        client.set_client_name("ozmux-0".to_string());
        client.set_per_window_refresh(true);
        assert_eq!(client.client_name(), Some("ozmux-0"));
        assert_eq!(client.supports_per_window_refresh(), Some(true));
    }
}
