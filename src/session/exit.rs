//! Child-process exit observer: sends `AppExit` when the shell quits.

use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use orzma_tty_engine::TerminalChildExit;

/// Registers the shell-exit observer.
pub(super) struct ExitPlugin;

impl Plugin for ExitPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_child_exit);
    }
}

fn on_child_exit(
    ev: On<TerminalChildExit>,
    mut exit: MessageWriter<AppExit>,
    terminals: Query<(), With<OrzmaTerminal>>,
) {
    if terminals.get(ev.event_target()).is_ok() {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::message::MessageReader;
    use orzma_tty_engine::TerminalChildExit;

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

        let entity = app.world_mut().spawn(OrzmaTerminal).id();
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
