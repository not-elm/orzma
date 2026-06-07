//! `MultiplexerPlugin` wires the multiplexer into a Bevy `App`. Add this
//! plugin to any `App` that uses `MultiplexerCommands`.

#[cfg(not(feature = "thin-client"))]
use crate::commands::WorkspaceNameCounter;
#[cfg(not(feature = "thin-client"))]
use crate::mirror::{MultiplexerStartupSet, MuxState, materialize_mux_snapshot};
use bevy::prelude::*;

/// Bevy plugin that initializes the `WorkspaceNameCounter` resource that
/// `MultiplexerCommands` consumes, inserts the authoritative `MuxState`
/// resource, and runs the Startup materialize system.
/// Required for correct `MultiplexerCommands` use.
/// In the `thin-client` build, this plugin is a no-op; the app crate wires
/// its own init and does not add this plugin.
pub struct MultiplexerPlugin;

impl Plugin for MultiplexerPlugin {
    fn build(&self, _app: &mut App) {
        #[cfg(not(feature = "thin-client"))]
        {
            _app.init_resource::<WorkspaceNameCounter>();
            _app.insert_resource(MuxState::new(ozmux_mux::Mux::new()));
            _app.add_systems(
                Startup,
                materialize_mux_snapshot.in_set(MultiplexerStartupSet::Materialize),
            );
            #[cfg(debug_assertions)]
            _app.add_systems(PostUpdate, crate::mirror::assert_mirror_consistent);
        }
    }
}
