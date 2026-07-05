use crate::{
    action::{terminal::PasteAction, vi::trigger_copy_mode_action},
    app_mode::AppMode,
    input::{
        keyboard::bevy_key_to_terminal_key,
        shortcuts::{CopyModeMessage, ShortcutMessage, ShortcutSet, TypeMessage},
    },
    ui::copy_mode::EnterCopyModeActionEvent,
};
use bevy::prelude::*;
use ozma_tty_engine::{TerminalKeyInput, TerminalModifiers};
use ozmux_configs::shortcuts::Shortcut;

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
                apply_default_copy_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<CopyModeMessage>),
                apply_default_type
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<TypeMessage>)
                    .after(apply_default_shortcuts)
                    .after(apply_default_copy_mode),
            ),
        );
    }
}

/// Applies `AppMode::Default` keyboard shortcuts from `ShortcutMessage`:
/// copy-mode entry and paste (direct paste fires outside copy mode; a leader
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
            Shortcut::EnterCopyMode => {
                if let Some(entity) = msg.focused {
                    commands.trigger(EnterCopyModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused
                    && (msg.via_leader || !msg.in_copy_mode)
                {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::DetachSession
            | Shortcut::SelectPane(_)
            | Shortcut::SplitPane(_)
            | Shortcut::KillPane
            | Shortcut::ZoomPane
            | Shortcut::NewWindow
            | Shortcut::KillWindow
            | Shortcut::NextWindow
            | Shortcut::PreviousWindow
            | Shortcut::SelectWindow(_)
            | Shortcut::RenameWindow
            | Shortcut::RenameSession
            | Shortcut::Quit
            | Shortcut::ReleaseWebviewFocus => {}
        }
    }
}

/// Applies matched `[copy-mode]` keys from `CopyModeMessage` on the focused
/// terminal. Registered in `ShortcutSet::Apply`, gated on
/// `in_state(AppMode::Default)` + `on_message::<CopyModeMessage>`.
pub(in crate::input) fn apply_default_copy_mode(
    mut commands: Commands,
    mut copy_mode: MessageReader<CopyModeMessage>,
) {
    for msg in copy_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_copy_mode_action(&mut commands, entity, msg.action);
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
