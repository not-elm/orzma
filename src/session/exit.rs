//! Session end: `MuxSessionEnded` (the last pane closed, or the backend
//! is gone) sends `AppExit`.

use bevy::prelude::*;
use bevy_orzma_mux::prelude::MuxSessionEnded;

/// Registers the session-end observer.
pub(super) struct ExitPlugin;

impl Plugin for ExitPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_session_ended);
    }
}

fn on_session_ended(_ev: On<MuxSessionEnded>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::MessageReader;

    /// Asserts that a `MuxSessionEnded` event sends `AppExit`.
    ///
    /// Case: the last pane's shell exits, or the backend thread panics.
    #[test]
    fn session_end_sends_app_exit() {
        #[derive(Resource, Default)]
        struct GotExit(bool);

        fn capture(mut reader: MessageReader<AppExit>, mut flag: ResMut<GotExit>) {
            if reader.read().next().is_some() {
                flag.0 = true;
            }
        }

        let mut app = App::new();
        app.add_message::<AppExit>();
        app.add_observer(on_session_ended);
        app.init_resource::<GotExit>();
        app.add_systems(Update, capture);

        app.world_mut().trigger(MuxSessionEnded);
        app.update();

        assert!(
            app.world().resource::<GotExit>().0,
            "AppExit should have been sent on MuxSessionEnded",
        );
    }
}
