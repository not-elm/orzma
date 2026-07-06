//! `AppMode::Default`'s shortcut appliers: reads `ShortcutMessage`,
//! `ViModeMessage`, and `TypeMessage` from `resolve_key_effects` and applies
//! vi-mode entry, paste, and raw-key typing to the focused terminal.

use crate::{
    action::{terminal::PasteAction, vi::trigger_vi_mode_action},
    app_mode::AppMode,
    input::{
        keyboard::bevy_key_to_terminal_key,
        shortcuts::{ShortcutMessage, ShortcutSet, TypeMessage, ViModeMessage},
    },
    ui::vi_mode::EnterViModeActionEvent,
};
use bevy::prelude::*;
use orzma_configs::shortcuts::Shortcut;
use orzma_tty_engine::{TerminalKeyInput, TerminalModifiers};

pub(super) struct ShortcutsDefaultModePlugin;

impl Plugin for ShortcutsDefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_default_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<ShortcutMessage>),
                apply_default_vi_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<ViModeMessage>)
                    .after(apply_default_shortcuts),
                apply_default_type
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<TypeMessage>)
                    .after(apply_default_shortcuts)
                    .after(apply_default_vi_mode),
            ),
        );
    }
}

/// Applies `AppMode::Default` keyboard shortcuts from `ShortcutMessage`:
/// vi-mode entry and paste (direct paste fires outside vi mode; a leader
/// paste fires unconditionally). `Quit` / `ReleaseWebviewFocus` are handled
/// upstream in `resolve_key_effects`; pane/window actions are no-ops in Default.
/// Registered in `ShortcutSet::Apply`, gated on `in_state(AppMode::Default)` +
/// `on_message::<ShortcutMessage>`.
pub(in crate::input) fn apply_default_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::EnterViMode => {
                if let Some(entity) = msg.focused {
                    commands.trigger(EnterViModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused
                    && (msg.via_leader || !msg.in_vi_mode)
                {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::DetachSession
            | Shortcut::SelectPane(_)
            | Shortcut::ResizePane(_)
            | Shortcut::SplitPane(_)
            | Shortcut::KillPane
            | Shortcut::ZoomPane
            | Shortcut::NewWindow
            | Shortcut::KillWindow
            | Shortcut::NextWindow
            | Shortcut::PreviousWindow
            | Shortcut::NextSession
            | Shortcut::PreviousSession
            | Shortcut::SelectWindow(_)
            | Shortcut::RenameWindow
            | Shortcut::RenameSession
            | Shortcut::Quit
            | Shortcut::ReleaseWebviewFocus => {}
        }
    }
}

/// Applies matched `[vi-mode]` keys from `ViModeMessage` on the focused
/// terminal. Registered in `ShortcutSet::Apply`, gated on
/// `in_state(AppMode::Default)` + `on_message::<ViModeMessage>`.
pub(in crate::input) fn apply_default_vi_mode(
    mut commands: Commands,
    mut vi_mode: MessageReader<ViModeMessage>,
) {
    for msg in vi_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_vi_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Types raw keys from `TypeMessage` into the focused terminal as
/// `TerminalKeyInput`. Runs after the shortcut/copy appliers. Registered in
/// `ShortcutSet::Apply`, gated on `in_state(AppMode::Default)` +
/// `on_message::<TypeMessage>`.
pub(in crate::input) fn apply_default_type(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
) {
    for msg in type_keys.read() {
        if let Some(entity) = msg.focused
            && let Some(key) = bevy_key_to_terminal_key(&msg.logical)
        {
            let terminal_mods = TerminalModifiers {
                ctrl: msg.mods.ctrl,
                shift: msg.mods.shift,
                alt: msg.mods.alt,
                meta: msg.mods.meta,
            };
            commands.trigger(TerminalKeyInput {
                entity,
                key,
                modifiers: terminal_mods,
            });
        }
    }
}
