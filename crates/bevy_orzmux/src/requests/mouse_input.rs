//! A mouse-protocol report the host UI asks a terminal entity to
//! receive, sent as `OrzmuxCommand::MouseInput`.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
use orzma_tty::prelude::MouseReport;
use orzmux::prelude::OrzmuxCommand;

/// Fired by the host UI to forward one mouse-protocol report to a specific
/// terminal entity.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyMouseInput {
    #[event_target]
    pub terminal: Entity,
    /// The report, encoded at apply time against the terminal's active
    /// mouse encoding.
    pub mouse: MouseReport,
}

/// Forwards [`RequestTtyMouseInput`] to the addressed pane.
pub(super) struct MouseInputPlugin;

impl Plugin for MouseInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_mouse_input.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_mouse_input(e: On<RequestTtyMouseInput>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::MouseInput {
        pane,
        report: e.mouse,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_tty::prelude::{CellCoord, MouseButton, MouseReportKind, ProtocolModifiers};
    use orzmux::prelude::PaneId;

    fn report() -> MouseReport {
        MouseReport {
            button: MouseButton::Left,
            kind: MouseReportKind::Press,
            cell: CellCoord { col: 1, row: 1 },
            mods: ProtocolModifiers::default(),
        }
    }

    /// Asserts that a mouse request for a pane entity becomes a
    /// `MouseInput` command for that pane id, and one for a non-pane
    /// entity sends nothing.
    ///
    /// Case: the user clicks over a pane's grid, then over the
    /// separator between two panes.
    #[test]
    fn mouse_requests_become_mouse_input_commands_for_the_pane() {
        let (mut app, commands) = app_with_connection(MouseInputPlugin);
        let pane = spawn_pane(&mut app, PaneId(2));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtyMouseInput {
            terminal: pane,
            mouse: report(),
        });
        app.world_mut().trigger(RequestTtyMouseInput {
            terminal: stray,
            mouse: report(),
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::MouseInput {
                pane: PaneId(2),
                ..
            }
        ));
    }
}
