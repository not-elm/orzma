//! Makes `Time<Real>` read the system clock at the start of each update.

use bevy::prelude::*;
use bevy::time::TimeReceiver;

/// Makes `Time<Real>` read the system clock at `First`, rather than the
/// instant the render world sent after the previous render.
pub(super) struct LiveClockPlugin;

impl Plugin for LiveClockPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        app.world_mut().remove_resource::<TimeReceiver>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::create_time_channels;

    /// Asserts that finishing the app removes the render world's time
    /// channel.
    ///
    /// Case: orzma starts with the render plugin, which installs the render
    /// world's time channel.
    #[test]
    fn finishing_the_app_removes_the_render_time_channel() {
        let mut app = App::new();
        let (_sender, receiver) = create_time_channels();
        app.insert_resource(receiver).add_plugins(LiveClockPlugin);
        app.finish();
        assert!(!app.world().contains_resource::<TimeReceiver>());
    }
}
