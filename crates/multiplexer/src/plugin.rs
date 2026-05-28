//! `MultiplexerPlugin` registers the two lifecycle observers needed for
//! dangling-reference cleanup. Add this plugin to any `App` that uses
//! `MultiplexerCommands` — without it, `ActivePane` / `ActiveActivity`
//! pointers will leak after Pane / Activity despawns.

use bevy::prelude::*;
use crate::observers::{on_remove_activity_marker, on_remove_pane_marker};

/// Bevy plugin that registers the multiplexer's dangling-reference
/// cleanup observers. Required for correct `MultiplexerCommands` use.
pub struct MultiplexerPlugin;

impl Plugin for MultiplexerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_remove_pane_marker);
        app.add_observer(on_remove_activity_marker);
    }
}
