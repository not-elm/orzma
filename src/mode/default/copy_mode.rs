//! In-copy-mode key handling for `AppMode::Default`: a gather system that
//! reads `KeyboardInput` for the focused copy-mode terminal, matches each key
//! against the config-driven shared key table (`ResolvedCopyModeKeys`), and
//! fires the shared VI events for `vi/default_mode.rs` to apply. Entry
//! (`Cmd+S`) lives in `app_shortcut_handler` (`src/mode/default/input.rs`);
//! this module owns only the keys handled WHILE copy mode is active.

use crate::action::vi::{ResolvedCopyModeKeys, trigger_copy_mode_action};
use crate::input::InputPhase;
use crate::input::current_modifiers;
use crate::input::default_mode::should_disable_input;
use crate::input::focus::KeyboardFocused;
use crate::input::ime::ImeState;
use crate::mode::AppMode;
use crate::ui::copy_mode::CopyModeState;
use bevy::ecs::message::MessageReader;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::OzmaTerminal;

/// Registers the in-copy-mode key gather system.
pub(crate) struct CopyModeInputPlugin;

impl Plugin for CopyModeInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            copy_mode_keys
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Default))
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

/// Gather system: reads `KeyboardInput` for the focused copy-mode terminal,
/// resolves each key against the config-driven table, and triggers the
/// matched VI event. Suspends (and drains events) while input is disabled
/// (IME composing, webview focused, or window unfocused) — the same coarse
/// guard `maintain_input_gates` uses (`should_disable_input`).
fn copy_mode_keys(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    resolved: Res<ResolvedCopyModeKeys>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminal: Query<
        Entity,
        (
            With<OzmaTerminal>,
            With<KeyboardFocused>,
            With<CopyModeState>,
        ),
    >,
) {
    let Ok(entity) = terminal.single() else {
        events.clear();
        return;
    };
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if should_disable_input(ime.is_composing(), focused, focused_webview.0.is_some()) {
        events.clear();
        return;
    }
    let mods = current_modifiers(&bevy_keys);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(action) = resolved.resolve(&ev.logical_key, ev.key_code, mods) {
            trigger_copy_mode_action(&mut commands, entity, action);
        }
    }
}
