//! Multiplexer Bevy Resource + per-session Component re-exports, plus a
//! Plugin that registers the Resource on app startup.

use bevy::prelude::*;
use ozmux_multiplexer::MultiplexerService;

/// `Action` → `MultiplexerService` mutation helpers consumed by the shortcut dispatcher.
pub mod commands;
/// Layout-change logging system + `render_tree` formatter for the `Multiplexer` Resource.
pub mod log;

pub use crate::session_entity::{AttachedSession, SessionEntityId, SessionUiSubtree};

/// Bevy Resource wrapping the in-memory `MultiplexerService`. `Deref` /
/// `DerefMut` let call sites invoke `MultiplexerService` methods directly.
#[derive(Resource, Default, Deref, DerefMut)]
pub struct Multiplexer(pub MultiplexerService);

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
        let (_sid, _pid, _aid) = mux.create_session(Some("test".into()));
        assert_eq!(mux.sessions.len(), 1);
    }
}
