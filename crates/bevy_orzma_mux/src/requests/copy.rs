//! `RequestTtyCopySelection`: asks the backend for a pane's selected
//! text; the answer arrives as `TtySelectionTextSignal`.

use crate::registry::PaneRegistry;
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
    mut registry: ResMut<PaneRegistry>,
    connection: Res<MuxConnection>,
    panes: Query<&MuxPane>,
) {
    let Ok(pane) = panes.get(e.terminal) else {
        return;
    };
    let request = RequestId::next();
    registry.pending_copies.insert(request, e.terminal);
    connection.0.send(MuxCommand::CopySelection {
        pane: PaneTarget::Id(pane.0),
        request,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_mux::prelude::PaneId;

    /// Asserts that a copy request registers its pending entity and sends
    /// `CopySelection` addressed by pane id.
    ///
    /// Case: the user presses Cmd+C with a selection in the focused pane.
    #[test]
    fn copy_requests_register_the_requester_and_send_copy_selection() {
        let (mut app, commands) = app_with_connection(CopyPlugin);
        let pane = spawn_pane(&mut app, PaneId(5));
        app.world_mut()
            .trigger(RequestTtyCopySelection { terminal: pane });
        let sent = sent(&commands);
        let [
            MuxCommand::CopySelection {
                pane: PaneTarget::Id(PaneId(5)),
                request,
            },
        ] = sent.as_slice()
        else {
            panic!("expected one CopySelection, got {sent:?}");
        };
        assert_eq!(
            app.world()
                .resource::<PaneRegistry>()
                .pending_copies
                .get(request),
            Some(&pane)
        );
    }
}
