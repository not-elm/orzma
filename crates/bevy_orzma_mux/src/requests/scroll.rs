//! `RequestTtyScroll`: the viewport movement the host UI asks a terminal
//! entity to perform, sent as `MuxCommand::Scroll`.

use crate::{MuxConnection, MuxPane};
use bevy::prelude::*;
use orzma_mux::prelude::MuxCommand;
use orzma_vt::prelude::Scroll;

/// Fired by the host UI to move a specific terminal entity's viewport.
///
/// The motion vocabulary is [`Scroll`] itself — the request carries
/// exactly what the backend applies, and the observer's only job is
/// routing it to the targeted entity's pane. Clamping at both ends of
/// history and page-size resolution live in `orzma_vt` and are pinned
/// by its tests, not re-asserted here.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyScroll {
    #[event_target]
    pub terminal: Entity,
    /// The movement to perform.
    pub scroll: Scroll,
}

pub(super) struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_scroll);
    }
}

fn apply_scroll(e: On<RequestTtyScroll>, connection: Res<MuxConnection>, panes: Query<&MuxPane>) {
    if let Ok(pane) = panes.get(e.terminal) {
        connection.0.send(MuxCommand::Scroll {
            pane: pane.0,
            scroll: e.scroll,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_mux::prelude::PaneId;

    /// Asserts that a scroll request for a pane entity becomes a
    /// `Scroll` command for that pane id, and one for a non-pane entity
    /// sends nothing.
    ///
    /// Case: the user turns the wheel over a pane, then over the
    /// separator.
    #[test]
    fn scroll_requests_become_scroll_commands_for_the_pane() {
        let (mut app, commands) = app_with_connection(ScrollPlugin);
        let pane = spawn_pane(&mut app, PaneId(2));
        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTtyScroll {
            terminal: pane,
            scroll: Scroll::Delta(3),
        });
        app.world_mut().trigger(RequestTtyScroll {
            terminal: stray,
            scroll: Scroll::Delta(1),
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            MuxCommand::Scroll {
                pane: PaneId(2),
                scroll: Scroll::Delta(3)
            }
        ));
    }
}
