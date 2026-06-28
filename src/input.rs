//! Root of the host input pipeline: keyboard, mouse, focus, gesture and binding
//! primitives, IME, hyperlink-hover, shortcuts, option-as-alt, and the
//! per-mode (`default_mode`, `tmux`) dispatchers.

mod bindings;
pub(crate) mod default_mode;
pub(crate) mod focus;
mod gesture;
pub(crate) mod hyperlink;
pub(crate) mod ime;
pub(crate) mod keyboard;
pub(crate) mod mouse;
pub(crate) mod option_as_alt;
pub(crate) mod shortcuts;
pub(crate) mod tmux;

use crate::system_set::OzmuxSystems;
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
    /// (`forward_keys_to_tmux`) run in this slot, after `Dispatch` has applied
    /// any IME events so the forwarder sees fresh `ImeState`.
    FocusedKey,
}

/// Bevy Plugin that registers the keyboard shortcut handling pipeline.
pub struct OzmuxShortcutPlugin;

impl Plugin for OzmuxShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<shortcuts::ResolvedShortcuts>()
            .add_systems(
                Startup,
                (
                    shortcuts::build_resolved_shortcuts,
                    shortcuts::populate_input_bindings,
                    shortcuts::populate_mouse_config,
                )
                    .chain(),
            )
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
