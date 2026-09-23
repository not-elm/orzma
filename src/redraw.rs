//! Runs the app's updates on demand, and wakes the app when something
//! needs drawing.

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use caret_wake::CaretWakePlugin;
use clock::LiveClockPlugin;
use follow_up::FollowUpPlugin;
use update_policy::UpdatePolicyPlugin;
pub(crate) use wake::AppWakers;

mod caret_wake;
mod clock;
mod follow_up;
mod update_policy;
mod wake;

/// Runs the app's updates on demand: updates only when woken or on input,
/// requests one follow-up frame after outside input, wakes the app for the
/// caret blink, and ticks at about 30 Hz while a webview exists.
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
            CaretWakePlugin::new(self.wakers.timer().clone()),
            UpdatePolicyPlugin,
        ))
        .add_systems(Last, trace_update);
    }
}

/// Logs each update at trace level under the `orzma::redraw` target.
fn trace_update(frame: Res<FrameCount>) {
    trace!(target: "orzma::redraw", frame = frame.0, "update");
}
