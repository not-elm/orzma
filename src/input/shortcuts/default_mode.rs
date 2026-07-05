use crate::{
    action::{terminal::PasteAction, vi::trigger_copy_mode_action},
    app_mode::AppMode,
    input::{
        keyboard::{bevy_key_to_terminal_key, key_effect::KeyEffect},
        shortcuts::{ShortcutBatch, ShortcutSet},
    },
    ui::copy_mode::EnterCopyModeActionEvent,
};
use bevy::prelude::*;
use ozma_tty_engine::{TerminalKeyInput, TerminalModifiers};
use ozmux_configs::shortcuts::ShortcutAction;

pub(super) struct ShortcutsDefaultModePlugin;

impl Plugin for ShortcutsDefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            apply_default_shortcuts
                .in_set(ShortcutSet::Apply)
                .run_if(in_state(AppMode::Default))
                .run_if(on_message::<ShortcutBatch>),
        );
    }
}

/// Applies `AppMode::Default` keyboard shortcuts from the frame's
/// `ShortcutBatch` (produced by `crate::input::dispatch::resolve_shortcuts`):
/// triggers the matching events on `batch.focused` — copy-mode entry, paste
/// (direct fires outside copy mode, leader fires unconditionally), the shared
/// `[copy-mode]` key table, and raw-key typing. `Quit` and
/// `ReleaseWebviewFocus` are handled upstream in `resolve_shortcuts`; the
/// pane/window actions are no-ops in Default mode. Registered in
/// `ShortcutSet::Apply`, gated on `in_state(AppMode::Default)` +
/// `on_message::<ShortcutBatch>`.
pub(in crate::input) fn apply_default_shortcuts(
    mut commands: Commands,
    mut batches: MessageReader<ShortcutBatch>,
) {
    for batch in batches.read() {
        let terminal_mods = TerminalModifiers {
            ctrl: batch.mods.ctrl,
            shift: batch.mods.shift,
            alt: batch.mods.alt,
            meta: batch.mods.meta,
        };
        for effect in &batch.effects {
            match effect {
                KeyEffect::Action {
                    action: ShortcutAction::EnterCopyMode,
                    ..
                } => {
                    if let Some(entity) = batch.focused {
                        commands.trigger(EnterCopyModeActionEvent { entity });
                    }
                }
                KeyEffect::Action {
                    action: ShortcutAction::Paste,
                    via_leader,
                } => {
                    if let Some(entity) = batch.focused
                        && (*via_leader || !batch.in_copy_mode)
                    {
                        commands.trigger(PasteAction { entity });
                    }
                }
                KeyEffect::Action {
                    action:
                        ShortcutAction::DetachSession
                        | ShortcutAction::SelectPane(_)
                        | ShortcutAction::SplitPane(_)
                        | ShortcutAction::KillPane
                        | ShortcutAction::ZoomPane
                        | ShortcutAction::NewWindow
                        | ShortcutAction::KillWindow
                        | ShortcutAction::NextWindow
                        | ShortcutAction::PreviousWindow
                        | ShortcutAction::SelectWindow(_)
                        | ShortcutAction::RenameWindow
                        | ShortcutAction::RenameSession
                        | ShortcutAction::Quit
                        | ShortcutAction::ReleaseWebviewFocus,
                    ..
                } => {}
                KeyEffect::CopyMode(action) => {
                    if let Some(entity) = batch.focused {
                        trigger_copy_mode_action(&mut commands, entity, *action);
                    }
                }
                KeyEffect::Type { logical, .. } => {
                    if let Some(entity) = batch.focused
                        && let Some(key) = bevy_key_to_terminal_key(logical)
                    {
                        commands.trigger(TerminalKeyInput {
                            entity,
                            key,
                            modifiers: terminal_mods,
                        });
                    }
                }
                KeyEffect::WebviewForward { .. } => {}
            }
        }
    }
}
