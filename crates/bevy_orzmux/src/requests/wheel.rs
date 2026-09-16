//! One frame's wheel notches the host UI asks a terminal entity to
//! route, sent as `OrzmuxCommand::Wheel`.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
use orzma_tty::prelude::WheelInput;
use orzmux::prelude::OrzmuxCommand;

/// Hands one frame's wheel notches to a specific terminal entity, which
/// routes them by its own modes.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyWheel {
    #[event_target]
    pub terminal: Entity,
    /// The notches and the modifiers and cell they were gathered with.
    pub input: WheelInput,
}

/// Forwards [`RequestTtyWheel`] to the addressed pane.
pub(super) struct WheelPlugin;

impl Plugin for WheelPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_wheel.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_wheel(e: On<RequestTtyWheel>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::Wheel {
        pane,
        input: e.input,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_tty::prelude::{CellCoord, ProtocolModifiers, WheelModifiers};
    use orzmux::prelude::PaneId;

    fn input() -> WheelInput {
        WheelInput {
            up: 2,
            right: 0,
            mods: WheelModifiers::default(),
            cell: Some(CellCoord { col: 1, row: 1 }),
            report_mods: ProtocolModifiers::default(),
        }
    }

    /// Asserts that a wheel request for a pane entity becomes a `Wheel`
    /// command for that pane id carrying the same input, and one for a
    /// non-pane entity sends nothing.
    ///
    /// Case: the user spins the wheel over a pane's grid, then over the
    /// separator between two panes.
    #[test]
    fn wheel_requests_become_wheel_commands_for_the_pane() {
        let (mut app, commands) = app_with_connection(WheelPlugin);
        let pane = spawn_pane(&mut app, PaneId(2));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtyWheel {
            terminal: pane,
            input: input(),
        });
        app.world_mut().trigger(RequestTtyWheel {
            terminal: stray,
            input: input(),
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            &sent[0],
            OrzmuxCommand::Wheel {
                pane: PaneId(2),
                input: sent_input,
            } if *sent_input == input()
        ));
    }
}
