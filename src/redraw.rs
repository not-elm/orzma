//! Runs the app's updates on demand, and wakes the app when something
//! needs drawing.

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use clock::LiveClockPlugin;
use follow_up::FollowUpPlugin;
pub(crate) use wake::AppWakers;

mod clock;
mod follow_up;
mod wake;

/// Runs the app's updates on demand: requests one follow-up frame after
/// outside input.
pub(crate) struct RedrawPlugin {
    wakers: AppWakers,
}

impl RedrawPlugin {
    /// Builds the plugin around the app's wake handles.
    pub fn new(wakers: AppWakers) -> Self {
        Self { wakers }
    }
}

impl Plugin for RedrawPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            LiveClockPlugin,
            FollowUpPlugin::new(self.wakers.gate().clone()),
        ))
        .add_systems(Last, trace_update);
    }
}

/// Logs each update at trace level under the `orzma::redraw` target.
fn trace_update(frame: Res<FrameCount>) {
    trace!(target: "orzma::redraw", frame = frame.0, "update");
}
