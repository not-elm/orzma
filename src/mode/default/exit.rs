//! Child-process exit observer: sends `AppExit` when the shell quits.

use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::TerminalChildExit;

/// Registers the shell-exit observer.
pub(super) struct DefaultExitPlugin;

impl Plugin for DefaultExitPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_child_exit);
    }
}

// NOTE: not Default-only despite the module path — detached tmux panes never
// emit TerminalChildExit, but the adopted tmux gateway keeps OzmaTerminal and
// a real PtyHandle, so this observer also fires (alongside
// on_gateway_child_exit) when the gateway shell dies during tmux mode.
fn on_child_exit(
    ev: On<TerminalChildExit>,
    mut exit: MessageWriter<AppExit>,
    terminals: Query<(), With<OzmaTerminal>>,
) {
    if terminals.get(ev.event_target()).is_ok() {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::MessageReader;
    use ozma_terminal::OzmaTerminal;
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
