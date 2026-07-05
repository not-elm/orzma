//! Root of the host input pipeline: keyboard, mouse, focus, gesture and binding
//! primitives, IME, hyperlink-hover, shortcuts, option-as-alt, and the
//! per-mode (`default_mode`, `tmux`) dispatchers.

mod bindings;
pub(crate) mod default_mode;
mod dispatch;
pub(crate) mod focus;
mod gesture;
pub(crate) mod hyperlink;
pub(crate) mod ime;
pub(crate) mod keyboard;
pub(crate) mod mouse;
pub(crate) mod option_as_alt;
mod resolve;
pub(crate) mod shortcuts;
pub(crate) mod tmux;

use crate::{
    input::{
        default_mode::DefaultHostInputPlugin, dispatch::DispatchPlugin,
        keyboard::KeyboardInputPlugin, mouse::MouseInputPlugin, option_as_alt::OptionAsAltPlugin,
        shortcuts::ShortcutsPlugin, tmux::TmuxInputPlugin,
    },
    system_set::OzmuxSystems,
};
use bevy::prelude::*;
use ozmux_configs::shortcuts::Modifiers;

/// Sub-phases of `OzmuxSystems::Input`. Runs in the order:
/// `Hover` (cursor / hyperlink hover detection) → `Dispatch`
/// (mouse / wheel button routing) → `FocusedKey` (keyboard
/// shortcut + key forwarding).
#[derive(SystemSet, Hash, PartialEq, Eq, Debug, Clone)]
pub(crate) enum InputPhase {
    Hover,
    Dispatch,
    /// Keyboard shortcut dispatch and tmux key forwarding
    /// (`apply_tmux_shortcuts`) run in this slot, after `Dispatch` has applied
    /// any IME events so the forwarder sees fresh `ImeState`.
    FocusedKey,
}

/// Bevy Plugin that registers the keyboard shortcut handling pipeline.
pub struct OzmuxInputPlugin;

impl Plugin for OzmuxInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ShortcutsPlugin,
            OptionAsAltPlugin,
            KeyboardInputPlugin,
            MouseInputPlugin,
            DispatchPlugin,
            TmuxInputPlugin,
            DefaultHostInputPlugin,
        ))
        .configure_sets(
            Update,
            (
                InputPhase::Hover,
                InputPhase::Dispatch,
                InputPhase::FocusedKey,
            )
                .chain()
                .in_set(OzmuxSystems::Input),
        );
    }
}

/// Returns the current modifier state from the `ButtonInput<KeyCode>` resource.
///
/// The result is stable within a single Update tick because `ButtonInput`
/// is updated in `PreUpdate`.
pub(crate) fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
    Modifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}
