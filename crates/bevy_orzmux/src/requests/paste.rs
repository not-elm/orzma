//! Paste for a specific pane or for the backend's active pane; both
//! become `OrzmuxCommand::Paste`.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
use orzmux::prelude::{OrzmuxCommand, PaneTarget};

/// Fired by the host UI to paste text into a specific terminal entity.
///
/// The host sends the clipboard text and nothing more; bracketed-paste
/// framing, marker stripping, and line-ending normalization happen when
/// the paste is applied.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyPaste {
    #[event_target]
    pub terminal: Entity,
    /// The text to paste, exactly as read from the clipboard.
    pub text: String,
}

/// Fired by the host UI to paste text into whichever pane is active
/// when the backend processes it.
#[derive(Event, Debug, Clone)]
pub struct RequestActivePaste {
    /// The text to paste, exactly as read from the clipboard.
    pub text: String,
}

/// Forwards [`RequestTtyPaste`] and [`RequestActivePaste`] to the
/// backend.
pub(super) struct PastePlugin;

impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_paste.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(apply_active_paste.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_paste(e: On<RequestTtyPaste>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::Paste {
        pane: PaneTarget::Id(pane),
        text: e.text.clone(),
    });
}

fn apply_active_paste(e: On<RequestActivePaste>, connection: Res<OrzmuxConnection>) {
    connection.0.send(OrzmuxCommand::Paste {
        pane: PaneTarget::Active,
        text: e.text.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzmux::prelude::PaneId;

    /// Asserts that an entity-addressed paste targets that pane by id
    /// and an active-addressed paste targets `Active`.
    ///
    /// Case: a webview forwards a paste to its host pane while the user
    /// pastes into the focused pane.
    #[test]
    fn entity_pastes_target_the_pane_id_and_active_pastes_target_active() {
        let (mut app, commands) = app_with_connection(PastePlugin);
        let entity = spawn_pane(&mut app, PaneId(4));
        app.world_mut().trigger(RequestTtyPaste {
            terminal: entity,
            text: "hello".into(),
        });
        app.world_mut().trigger(RequestActivePaste {
            text: "hello".into(),
        });
        let sent = sent(&commands);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::Paste {
                pane: PaneTarget::Id(PaneId(4)),
                ..
            }
        ));
        assert!(matches!(
            sent[1],
            OrzmuxCommand::Paste {
                pane: PaneTarget::Active,
                ..
            }
        ));
    }
}
