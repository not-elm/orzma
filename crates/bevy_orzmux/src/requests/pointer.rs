//! A pointer event the host UI asks a terminal entity to route, sent as
//! `OrzmuxCommand::Pointer`.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
use orzma_tty::prelude::PointerInput;
use orzmux::prelude::OrzmuxCommand;

/// Fired by the host UI to hand one pointer event to a specific terminal
/// entity, whose backend turns it into a mouse report or its own
/// selection by the pane's live modes.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyPointer {
    #[event_target]
    pub terminal: Entity,
    /// The pointer event.
    pub input: PointerInput,
}

/// Forwards [`RequestTtyPointer`] to the addressed pane.
pub(super) struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_pointer.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_pointer(e: On<RequestTtyPointer>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::Pointer {
        pane,
        input: e.input,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_tty::prelude::{CellCoord, PointerButton, PointerKind, ProtocolModifiers};
    use orzma_vt::prelude::CellSide;
    use orzmux::prelude::PaneId;

    fn press() -> PointerInput {
        PointerInput {
            kind: PointerKind::Press,
            button: Some(PointerButton::Left),
            cell: CellCoord { col: 1, row: 1 },
            side: CellSide::Left,
            click_count: 1,
            mods: ProtocolModifiers::default(),
        }
    }

    /// Asserts that a pointer request for a pane entity becomes a
    /// `Pointer` command for that pane id, and one for a non-pane entity
    /// sends nothing.
    ///
    /// Case: the user clicks over a pane's grid, then over the separator
    /// between two panes.
    #[test]
    fn pointer_requests_become_pointer_commands_for_the_pane() {
        let (mut app, commands) = app_with_connection(PointerPlugin);
        let pane = spawn_pane(&mut app, PaneId(2));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtyPointer {
            terminal: pane,
            input: press(),
        });
        app.world_mut().trigger(RequestTtyPointer {
            terminal: stray,
            input: press(),
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::Pointer {
                pane: PaneId(2),
                input,
            } if input == press()
        ));
    }
}
