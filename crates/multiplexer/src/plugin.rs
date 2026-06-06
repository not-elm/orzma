//! `MultiplexerPlugin` wires the multiplexer into a Bevy `App`. Add this
//! plugin to any `App` that uses `MultiplexerCommands`.

use crate::commands::WorkspaceNameCounter;
use crate::mirror::{MultiplexerStartupSet, MuxState, materialize_mux_snapshot};
use bevy::prelude::*;

/// Bevy plugin that initializes the `WorkspaceNameCounter` resource that
/// `MultiplexerCommands` consumes, inserts the authoritative `MuxState`
/// resource, and runs the Startup materialize system.
/// Required for correct `MultiplexerCommands` use.
pub struct MultiplexerPlugin;

impl Plugin for MultiplexerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceNameCounter>();
        app.insert_resource(MuxState::new(ozmux_mux::Mux::new()));
        app.add_systems(
            Startup,
            materialize_mux_snapshot.in_set(MultiplexerStartupSet::Materialize),
        );
        #[cfg(debug_assertions)]
        app.add_systems(PostUpdate, crate::mirror::assert_mirror_consistent);
    }
}
