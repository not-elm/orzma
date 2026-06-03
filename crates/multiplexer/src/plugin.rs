//! `MultiplexerPlugin` registers the two lifecycle observers needed for
//! dangling-reference cleanup. Add this plugin to any `App` that uses
//! `MultiplexerCommands` — without it, `ActivePane` / `ActiveSurface`
//! pointers will leak after Pane / Surface despawns.

use crate::commands::SessionNameCounter;
use crate::observers::{on_remove_pane_marker, on_remove_surface_marker};
use bevy::prelude::*;

/// Bevy plugin that registers the multiplexer's dangling-reference
/// cleanup observers and initializes the `SessionNameCounter` resource
/// that `MultiplexerCommands` consumes. Required for correct
/// `MultiplexerCommands` use.
pub struct MultiplexerPlugin;

impl Plugin for MultiplexerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SessionNameCounter>();
        app.add_observer(on_remove_pane_marker);
        app.add_observer(on_remove_surface_marker);
    }
}
