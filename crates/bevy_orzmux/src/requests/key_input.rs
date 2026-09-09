//! `RequestTtyKeyInput` (a specific pane) and `RequestActiveKeyInput`
//! (the backend's active pane): both become `OrzmuxCommand::KeyInput`.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
use orzma_tty::prelude::{TerminalKey, TerminalModifiers};
use orzmux::prelude::{OrzmuxCommand, PaneTarget};

/// A key for one specific pane entity (webview forwards and other
/// entity-addressed paths). Keyboard and IME input use
/// [`RequestActiveKeyInput`] instead so it resolves against the
/// backend's active pane in command order.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyKeyInput {
    #[event_target]
    pub terminal: Entity,
    /// The logical key pressed (character or named key, pre-encoding).
    pub key: TerminalKey,
    /// Modifier state at press time; feeds the encoder, not a raw HID state.
    pub modifiers: TerminalModifiers,
}

/// A key for whichever pane is active when the backend processes it.
#[derive(Event, Debug, Clone)]
pub struct RequestActiveKeyInput {
    /// The logical key pressed (character or named key, pre-encoding).
    pub key: TerminalKey,
    /// Modifier state at press time; feeds the encoder, not a raw HID state.
    pub modifiers: TerminalModifiers,
}

pub(super) struct KeyInputPlugin;

impl Plugin for KeyInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_key_input)
            .add_observer(apply_active_key_input);
    }
}

fn apply_key_input(e: On<RequestTtyKeyInput>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::KeyInput {
        pane: PaneTarget::Id(pane),
        key: e.key.clone(),
        mods: e.modifiers,
    });
}

fn apply_active_key_input(e: On<RequestActiveKeyInput>, connection: Res<OrzmuxConnection>) {
    connection.0.send(OrzmuxCommand::KeyInput {
        pane: PaneTarget::Active,
        key: e.key.clone(),
        mods: e.modifiers,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzma_tty::prelude::KeyText;
    use orzmux::prelude::PaneId;

    /// Asserts that an entity-addressed key targets that pane by id and
    /// an active-addressed key targets `Active`.
    ///
    /// Case: a webview forwards a chord to its host pane while the user
    /// types into the focused pane.
    #[test]
    fn entity_keys_target_the_pane_id_and_active_keys_target_active() {
        let (mut app, commands) = app_with_connection(KeyInputPlugin);
        let entity = spawn_pane(&mut app, PaneId(4));
        let key = TerminalKey::Character(KeyText::new("x").unwrap());
        app.world_mut().trigger(RequestTtyKeyInput {
            terminal: entity,
            key: key.clone(),
            modifiers: TerminalModifiers::default(),
        });
        app.world_mut().trigger(RequestActiveKeyInput {
            key,
            modifiers: TerminalModifiers::default(),
        });
        let sent = sent(&commands);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::KeyInput {
                pane: PaneTarget::Id(PaneId(4)),
                ..
            }
        ));
        assert!(matches!(
            sent[1],
            OrzmuxCommand::KeyInput {
                pane: PaneTarget::Active,
                ..
            }
        ));
    }
}
