//! Multiplexer Bevy Resource + AttachedSession Component, plus a Plugin that
//! registers the Resource on app startup.

use bevy::prelude::*;
use ozmux_multiplexer::{MultiplexerService, SessionId};

/// `Action` → `MultiplexerService` mutation helpers consumed by the shortcut dispatcher.
pub mod commands;
/// Layout-change logging system + `render_tree` formatter for the `Multiplexer` Resource.
pub mod log;

/// Bevy Resource wrapping the in-memory `MultiplexerService` (the single
/// source of truth for sessions / windows / panes / activities). `Deref` /
/// `DerefMut` let call sites invoke `MultiplexerService` methods directly.
#[derive(Resource, Default, Deref, DerefMut)]
pub struct Multiplexer(pub MultiplexerService);

/// Per-GUI-window Component pointing at which ozmux `Session` is currently
/// attached. Multiple GUI windows can attach to the same session (mirror) or
/// to different sessions (independent clients).
#[derive(Component, Debug, Clone)]
pub struct AttachedSession(pub SessionId);

/// Bevy Plugin that inserts the [`Multiplexer`] Resource at app build time.
pub struct OzmuxMultiplexerPlugin;

impl Plugin for OzmuxMultiplexerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Multiplexer>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_inserts_multiplexer_resource() {
        let mut app = App::new();
        app.add_plugins(OzmuxMultiplexerPlugin);
        assert!(app.world().get_resource::<Multiplexer>().is_some());
    }

    #[test]
    fn multiplexer_derefs_to_multiplexer_service() {
        let mut mux = Multiplexer::default();
        assert_eq!(mux.sessions.len(), 0);
        let _sid = mux.create_session(Some("test".into()));
        assert_eq!(mux.sessions.len(), 1);
    }
}
