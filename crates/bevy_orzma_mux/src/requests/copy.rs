//! `RequestTtyCopySelection`: asks the backend for a pane's selected
//! text; the answer arrives as `TtySelectionTextSignal`.

use crate::{MuxConnection, MuxPane};
use bevy::prelude::*;
use orzma_mux::prelude::{MuxCommand, PaneTarget, RequestId};

/// Copy the selection of one pane entity.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyCopySelection {
    #[event_target]
    pub terminal: Entity,
}

pub(super) struct CopyPlugin;

impl Plugin for CopyPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_copy_selection);
    }
}

fn apply_copy_selection(
    e: On<RequestTtyCopySelection>,
    connection: Res<MuxConnection>,
    panes: Query<&MuxPane>,
) {
    let Ok(pane) = panes.get(e.terminal) else {
        return;
    };
    connection.0.send(MuxCommand::CopySelection {
        pane: PaneTarget::Id(pane.0),
        request: RequestId::next(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_mux::prelude::PaneId;

    /// Asserts that a copy request sends `CopySelection` addressed by
    /// pane id.
    ///
    /// Case: the user presses Cmd+C with a selection in the focused pane.
    #[test]
    fn copy_requests_send_copy_selection() {
        let (mut app, commands) = app_with_connection(CopyPlugin);
        let pane = spawn_pane(&mut app, PaneId(5));
        app.world_mut()
            .trigger(RequestTtyCopySelection { terminal: pane });
        let sent = sent(&commands);
        assert!(
            matches!(
                sent.as_slice(),
                [MuxCommand::CopySelection {
                    pane: PaneTarget::Id(PaneId(5)),
                    ..
                }]
            ),
            "expected one CopySelection, got {sent:?}"
        );
    }
}
