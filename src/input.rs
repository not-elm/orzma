//! Root of the host input pipeline, turning window input events into
//! terminal input and UI focus state.

pub(crate) mod bindings;
pub(crate) mod focus;
mod hyperlink;
pub(crate) mod ime;
pub(crate) mod keyboard;
mod last_key;
pub(crate) mod mouse;
pub(crate) mod option_as_alt;
pub(crate) mod shortcuts;

use crate::{
    input::{
        focus::FocusSyncPlugin, hyperlink::HyperlinkInputPlugin, ime::ImePlugin,
        keyboard::KeyboardInputPlugin, last_key::LastKeyPlugin, mouse::MouseInputPlugin,
        option_as_alt::OptionAsAltPlugin, shortcuts::ShortcutsPlugin,
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
            LastKeyPlugin,
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

/// Returns `true` when the platform's hyperlink-activation modifier is
/// currently held: Cmd (`meta`) on macOS, Ctrl elsewhere.
fn link_modifier_held(mods: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.meta
    } else {
        mods.ctrl
    }
}

/// Test-only input: presses the platform's hyperlink-activation modifier
/// in `app`'s keyboard state.
#[cfg(test)]
fn hold_link_modifier(app: &mut App) {
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    if cfg!(target_os = "macos") {
        keys.press(KeyCode::SuperLeft);
    } else {
        keys.press(KeyCode::ControlLeft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Modifiers {
        Modifiers::default()
    }

    /// Asserts that the link modifier reads as released while no modifier
    /// key is held.
    ///
    /// Case: the user moves the pointer over a URL without holding any key.
    #[test]
    fn link_modifier_held_returns_false_when_no_modifier() {
        assert!(!link_modifier_held(&empty()));
    }

    /// Asserts that on macOS the link modifier is Cmd, not Ctrl.
    ///
    /// Case: a macOS user holds Ctrl over a URL, then adds Cmd.
    #[cfg(target_os = "macos")]
    #[test]
    fn link_modifier_held_macos_requires_meta() {
        let mut mods = empty();
        mods.ctrl = true;
        assert!(!link_modifier_held(&mods));
        mods.meta = true;
        assert!(link_modifier_held(&mods));
    }

    /// Asserts that off macOS the link modifier is Ctrl, not the Super key.
    ///
    /// Case: a Linux or Windows user holds the Super key over a URL, then
    /// adds Ctrl.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn link_modifier_held_non_macos_requires_ctrl() {
        let mut mods = empty();
        mods.meta = true;
        assert!(!link_modifier_held(&mods));
        mods.ctrl = true;
        assert!(link_modifier_held(&mods));
    }
}
