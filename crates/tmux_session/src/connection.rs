//! The `NonSend` resource that owns the live `tmux -CC` client.

use tmux_control::TmuxClient;

/// Owns the live `tmux -CC` connection, if any.
///
/// Held as a Bevy **`NonSend`** resource because [`TmuxClient`] is `Send`
/// but not `Sync` (it owns a `Box<dyn MasterPty + Send>`). Insert it with
/// `app.insert_non_send_resource(TmuxConnection::default())` and access it
/// via `NonSend<TmuxConnection>` / `NonSendMut<TmuxConnection>`.
#[derive(Default)]
pub struct TmuxConnection {
    client: Option<TmuxClient>,
}

impl TmuxConnection {
    /// Installs `client` as the live connection, replacing any prior one.
    pub fn set(&mut self, client: TmuxClient) {
        self.client = Some(client);
    }

    /// Returns the live client, or `None` when disconnected.
    pub fn client(&self) -> Option<&TmuxClient> {
        self.client.as_ref()
    }

    /// Removes and returns the live client, leaving the connection empty.
    pub fn take(&mut self) -> Option<TmuxClient> {
        self.client.take()
    }
}
