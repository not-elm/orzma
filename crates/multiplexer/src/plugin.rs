//! `MultiplexerPlugin` registers the two lifecycle observers needed for
//! dangling-reference cleanup. Add this plugin to any `App` that uses
//! `MultiplexerCommands` — without it, `ActivePane` / `ActiveSurface`
//! pointers will leak after Pane / Surface despawns.

use crate::commands::WorkspaceNameCounter;
use crate::mirror::{MultiplexerStartupSet, MuxState, materialize_mux_snapshot};
use crate::observers::{on_remove_pane_marker, on_remove_surface_marker};
use bevy::prelude::*;

/// Bevy plugin that registers the multiplexer's dangling-reference
/// cleanup observers, initializes the `WorkspaceNameCounter` resource
/// that `MultiplexerCommands` consumes, inserts the authoritative
/// `MuxState` resource, and runs the Startup materialize system.
/// Required for correct `MultiplexerCommands` use.
pub struct MultiplexerPlugin;

impl Plugin for MultiplexerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceNameCounter>();
        app.insert_resource(MuxState::new(ozmux_mux::Mux::new()));
        app.add_observer(on_remove_pane_marker);
        app.add_observer(on_remove_surface_marker);
        app.add_systems(
            Startup,
            materialize_mux_snapshot.in_set(MultiplexerStartupSet::Materialize),
        );
    }
}
