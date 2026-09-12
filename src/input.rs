//! Root of the host input pipeline, turning window input events into
//! terminal input and UI focus state.

mod bindings;
pub(crate) mod focus;
mod hyperlink;
pub(crate) mod ime;
pub(crate) mod keyboard;
pub(crate) mod mouse;
pub(crate) mod option_as_alt;
pub(crate) mod shortcuts;

use crate::{
    input::{
        focus::FocusSyncPlugin, hyperlink::HyperlinkInputPlugin, ime::ImePlugin,
        keyboard::KeyboardInputPlugin, mouse::MouseInputPlugin, option_as_alt::OptionAsAltPlugin,
        shortcuts::ShortcutsPlugin,
    },
    system_set::OrzmaSystems,
};
use bevy::prelude::*;
use orzma_configs::shortcuts::Modifiers;

/// Sub-phases of `OrzmaSystems::Input`. Runs in the order:
/// `Hover` (cursor / hyperlink hover detection) → `Dispatch`
/// (mouse / wheel button routing) → `FocusedKey` (keyboard
/// shortcut + key forwarding).
#[derive(SystemSet, Hash, PartialEq, Eq, Debug, Clone)]
pub(crate) enum InputPhase {
    Hover,
    Dispatch,
    /// Keyboard shortcut dispatch runs in this slot, after `Dispatch` has
    /// applied any IME events so the dispatcher sees fresh `ImeState`.
    FocusedKey,
}

/// The host input pipeline: keyboard, mouse, IME, focus gates, hyperlink
/// hover, and shortcuts.
pub struct OrzmaInputPlugin;

impl Plugin for OrzmaInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ShortcutsPlugin,
            OptionAsAltPlugin,
            KeyboardInputPlugin,
            MouseInputPlugin,
            FocusSyncPlugin,
            ImePlugin,
            HyperlinkInputPlugin,
        ))
        .configure_sets(
            Update,
            (
                InputPhase::Hover,
                InputPhase::Dispatch,
                InputPhase::FocusedKey,
            )
                .chain()
                .in_set(OrzmaSystems::Input),
        );
    }
}

/// Returns the current modifier state from the `ButtonInput<KeyCode>` resource.
///
/// The result is stable within a single Update tick.
pub(crate) fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
    Modifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}
