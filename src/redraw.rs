//! Runs the app's updates on demand, and wakes the app when something
//! needs drawing.

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use clock::LiveClockPlugin;

mod clock;

/// Runs the app's updates on demand.
pub(crate) struct RedrawPlugin;

impl Plugin for RedrawPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LiveClockPlugin)
            .add_systems(Last, trace_update);
    }
}

/// Logs each update at trace level under the `orzma::redraw` target.
fn trace_update(frame: Res<FrameCount>) {
    trace!(target: "orzma::redraw", frame = frame.0, "update");
}
