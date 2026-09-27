//! The vi-mode switch the host UI asks a terminal entity to perform.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
pub use orzma_vt::prelude::ViModeSwitch;
use orzmux::prelude::OrzmuxCommand;

/// Fired by the host UI to enter or leave vi mode on a specific terminal
/// entity.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyViMode {
    #[event_target]
    pub terminal: Entity,
    /// Which direction to switch.
    pub switch: ViModeSwitch,
}

pub(super) struct ViModePlugin;

impl Plugin for ViModePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_vi_mode.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_vi_mode(e: On<RequestTtyViMode>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::ViMode {
        pane,
        switch: e.switch,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzmux::prelude::{OrzmuxCommand, PaneId};

    /// Every `(target, switch)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, ViModeSwitch)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTtyViMode>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.switch));
    }

    /// Asserts that both switch directions reach an observer with their target
    /// intact, in the order they were fired.
    ///
    /// Case: the enter/exit round trip a user performs constantly
    /// (`Ctrl-Shift-Space` in, `Esc` out).
    #[test]
    fn trigger_delivers_both_switch_directions_in_order() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        for switch in [ViModeSwitch::Enter, ViModeSwitch::Exit] {
            app.world_mut()
                .trigger(RequestTtyViMode { terminal, switch });
        }

        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![
                (terminal, ViModeSwitch::Enter),
                (terminal, ViModeSwitch::Exit)
            ]
        );
    }

    /// Asserts that a repeated switch is delivered every time rather than
    /// deduplicated.
    ///
    /// Case: `Enter` fired while already in vi mode.
    #[test]
    fn a_repeated_switch_is_not_deduplicated() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        for _ in 0..2 {
            app.world_mut().trigger(RequestTtyViMode {
                terminal,
                switch: ViModeSwitch::Enter,
            });
        }

        assert_eq!(app.world().resource::<Seen>().0.len(), 2);
    }

    /// Asserts that a switch request becomes a `ViMode` command for the
    /// addressed pane, and one for a non-pane entity sends nothing.
    ///
    /// Case: the user enters vi mode on a pane while a stray request targets
    /// the separator entity.
    #[test]
    fn a_switch_request_becomes_a_vi_mode_command_for_the_pane() {
        let (mut app, commands) = app_with_connection(ViModePlugin);
        let pane = spawn_pane(&mut app, PaneId(4));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtyViMode {
            terminal: pane,
            switch: ViModeSwitch::Enter,
        });
        app.world_mut().trigger(RequestTtyViMode {
            terminal: stray,
            switch: ViModeSwitch::Enter,
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::ViMode {
                pane: PaneId(4),
                switch: ViModeSwitch::Enter
            }
        ));
    }
}
