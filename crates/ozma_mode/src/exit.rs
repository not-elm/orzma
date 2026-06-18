//! Child-process exit observer: sends `AppExit` when the shell quits.

use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::TerminalChildExit;

/// Observer fired when the PTY child process exits.
///
/// Sends `AppExit::Success` only when the exiting terminal is the Ozma
/// terminal — ignores exits from any other terminal entity in the world.
pub(crate) fn on_child_exit(
    ev: On<TerminalChildExit>,
    mut exit: MessageWriter<AppExit>,
    terminal_q: Query<(), With<OzmaTerminal>>,
) {
    if terminal_q.get(ev.event_target()).is_ok() {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::MessageReader;
    use crate::spawn::OzmaTerminal;
    use ozma_tty_engine::TerminalChildExit;

    #[test]
    fn child_exit_sends_app_exit() {
        #[derive(Resource, Default)]
        struct GotExit(bool);

        fn capture(mut reader: MessageReader<AppExit>, mut flag: ResMut<GotExit>) {
            if reader.read().next().is_some() {
                flag.0 = true;
            }
        }

        let mut app = App::new();
        app.add_message::<AppExit>();
        app.add_observer(on_child_exit);
        app.init_resource::<GotExit>();
        app.add_systems(Update, capture);

        let entity = app.world_mut().spawn(OzmaTerminal).id();
        app.world_mut().trigger(TerminalChildExit {
            entity,
            code: Some(0),
        });
        app.update();

        assert!(
            app.world().resource::<GotExit>().0,
            "AppExit should have been sent on TerminalChildExit",
        );
    }
}
