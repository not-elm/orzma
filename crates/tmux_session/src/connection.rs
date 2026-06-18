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
    client_name: Option<String>,
    per_window_refresh: Option<bool>,
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
    ///
    /// Also clears the cached client name and capability flags so a fresh
    /// reconnect re-queries them.
    pub fn take(&mut self) -> Option<TmuxClient> {
        self.client_name = None;
        self.per_window_refresh = None;
        self.client.take()
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

    /// Caches the per-window `refresh-client` capability derived from the tmux
    /// version reply.
    pub(crate) fn set_per_window_refresh(&mut self, supported: bool) {
        self.per_window_refresh = Some(supported);
    }
}
