use crate::{
    action::{
        terminal::PasteAction,
        tmux::{
            DetachSessionRequest, KillPaneRequest, KillWindowRequest, NewWindowRequest,
            NextWindowRequest, PreviousWindowRequest, RenameSessionRequest, RenameWindowRequest,
            SelectPaneRequest, SelectWindowRequest, SplitPaneRequest, ZoomPaneRequest,
        },
        vi::trigger_copy_mode_action,
    },
    app_mode::{AppMode, TmuxActiveSet},
    input::{
        keyboard::key_effect::KeyEffect,
        shortcuts::{ShortcutBatch, ShortcutSet},
        tmux::forward::ForwardPaneKeysRequest,
    },
    ui::copy_mode::EnterCopyModeActionEvent,
};
use bevy::{ecs::system::SystemParam, prelude::*};
use ozmux_configs::shortcuts::Shortcut;
use ozmux_configs::shortcuts::{
    PaneDirection as CfgPaneDirection, SplitOrientation as CfgSplitOrientation,
};
use ozmux_tmux::{
    ActiveWindow, KeyMods, PaneDirection, SplitDirection, TmuxSession, TmuxWindow,
    bevy_key_to_tmux_name,
};

pub(super) struct ShortcutsTmuxModePlugin;

impl Plugin for ShortcutsTmuxModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (apply_tmux_shortcuts
                .in_set(ShortcutSet::Apply)
                .run_if(in_state(AppMode::Tmux))
                .run_if(on_message::<ShortcutBatch>),)
                .in_set(TmuxActiveSet),
        );
    }
}

/// Target-entity lookups for the tmux shortcut actions, bundled to stay
/// within Bevy's system-parameter limit.
#[derive(SystemParam)]
pub(in crate::input) struct ActionTargets<'w, 's> {
    active_window: Query<'w, 's, Entity, With<ActiveWindow>>,
    session: Query<'w, 's, Entity, With<TmuxSession>>,
    windows: Query<'w, 's, (Entity, &'static TmuxWindow)>,
}

/// Applies `AppMode::Tmux` keyboard shortcuts from the frame's `ShortcutBatch`
/// (produced by `crate::input::dispatch::resolve_shortcuts`): triggers the
/// matching events on the active pane (`batch.focused`) / session / window —
/// copy-mode entry, paste (`PasteAction`, applied by `on_paste_tmux`), detach
/// (`DetachSessionRequest`), the pane/window action requests, the shared
/// `[copy-mode]` key table, and raw-key forwarding batched into one
/// `ForwardPaneKeysRequest` per frame. `Quit` and `ReleaseWebviewFocus` are
/// handled upstream in `resolve_shortcuts`. Registered in `ShortcutSet::Apply`,
/// gated on `in_state(AppMode::Tmux)` + `on_message::<ShortcutBatch>`.
pub(in crate::input) fn apply_tmux_shortcuts(
    mut commands: Commands,
    mut batches: MessageReader<ShortcutBatch>,
    targets: ActionTargets,
) {
    for batch in batches.read() {
        let kmods = KeyMods {
            ctrl: batch.mods.ctrl,
            shift: batch.mods.shift,
            alt: batch.mods.alt,
            super_: batch.mods.meta,
        };
        let mut names: Vec<String> = Vec::new();
        for effect in &batch.effects {
            match effect {
                KeyEffect::Shortcut {
                    action: Shortcut::EnterCopyMode,
                    ..
                } => {
                    // NOTE: re-entry guard — re-triggering while already in copy
                    // mode would double-insert CopyModeState and re-enter vi mode.
                    if let Some(entity) = batch.focused
                        && !batch.in_copy_mode
                    {
                        commands.trigger(EnterCopyModeActionEvent { entity });
                    }
                }
                KeyEffect::Shortcut {
                    action: Shortcut::Paste,
                    ..
                } => {
                    if let Some(entity) = batch.focused {
                        commands.trigger(PasteAction { entity });
                    }
                }
                KeyEffect::Shortcut {
                    action: Shortcut::DetachSession,
                    ..
                } => {
                    if let Ok(entity) = targets.session.single() {
                        commands.trigger(DetachSessionRequest { entity });
                    }
                }
                KeyEffect::Shortcut { action, .. } => {
                    dispatch_tmux_action(&mut commands, *action, batch.focused, &targets);
                }
                KeyEffect::CopyMode(action) => {
                    if let Some(entity) = batch.focused {
                        trigger_copy_mode_action(&mut commands, entity, *action);
                    }
                }
                KeyEffect::Type { logical, key_code }
                | KeyEffect::WebviewForward { logical, key_code } => {
                    if let Some(name) = bevy_key_to_tmux_name(logical, *key_code, kmods) {
                        names.push(name);
                    }
                }
            }
        }

        if let Some(entity) = batch.focused
            && !names.is_empty()
        {
            commands.trigger(ForwardPaneKeysRequest { entity, names });
        }
    }
}

/// Triggers the tmux pane/window action request for `action` on the resolved
/// target: pane actions on `active_entity`, window actions on the active window
/// or the display-indexed window, session-scoped actions on the session. The
/// non-tmux actions (handled by the caller before this helper) are no-ops.
fn dispatch_tmux_action(
    commands: &mut Commands,
    action: Shortcut,
    active_entity: Option<Entity>,
    targets: &ActionTargets,
) {
    match action {
        Shortcut::SelectPane(direction) => {
            if let Some(entity) = active_entity {
                commands.trigger(SelectPaneRequest {
                    entity,
                    direction: tmux_pane_direction(direction),
                });
            }
        }
        Shortcut::SplitPane(orientation) => {
            if let Some(entity) = active_entity {
                commands.trigger(SplitPaneRequest {
                    entity,
                    direction: tmux_split_direction(orientation),
                });
            }
        }
        Shortcut::KillPane => {
            if let Some(entity) = active_entity {
                commands.trigger(KillPaneRequest { entity });
            }
        }
        Shortcut::ZoomPane => {
            if let Some(entity) = active_entity {
                commands.trigger(ZoomPaneRequest { entity });
            }
        }
        Shortcut::NewWindow => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(NewWindowRequest { entity });
            }
        }
        Shortcut::NextWindow => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(NextWindowRequest { entity });
            }
        }
        Shortcut::PreviousWindow => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(PreviousWindowRequest { entity });
            }
        }
        Shortcut::SelectWindow(index) => {
            if let Some(entity) = targets
                .windows
                .iter()
                .find(|(_, window)| window.index == u32::from(index))
                .map(|(entity, _)| entity)
            {
                commands.trigger(SelectWindowRequest { entity });
            }
        }
        Shortcut::KillWindow => {
            if let Ok(entity) = targets.active_window.single() {
                commands.trigger(KillWindowRequest { entity });
            }
        }
        Shortcut::RenameWindow => {
            if let Ok(entity) = targets.active_window.single() {
                commands.trigger(RenameWindowRequest { entity });
            }
        }
        Shortcut::RenameSession => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(RenameSessionRequest { entity });
            }
        }
        Shortcut::Quit
        | Shortcut::EnterCopyMode
        | Shortcut::Paste
        | Shortcut::DetachSession
        | Shortcut::ReleaseWebviewFocus => {}
    }
}

/// Maps the config-facing pane direction (named after the neighbor) to the
/// tmux command enum.
pub(in crate::input) fn tmux_pane_direction(direction: CfgPaneDirection) -> PaneDirection {
    match direction {
        CfgPaneDirection::Left => PaneDirection::Left,
        CfgPaneDirection::Down => PaneDirection::Down,
        CfgPaneDirection::Up => PaneDirection::Up,
        CfgPaneDirection::Right => PaneDirection::Right,
    }
}

/// Maps the config-facing split orientation (named after the DIVIDER) to the
/// tmux flag enum (named after the layout axis) — the two cross on purpose.
pub(in crate::input) fn tmux_split_direction(orientation: CfgSplitOrientation) -> SplitDirection {
    match orientation {
        CfgSplitOrientation::Vertical => SplitDirection::Horizontal,
        CfgSplitOrientation::Horizontal => SplitDirection::Vertical,
    }
}
