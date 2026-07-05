//! `AppMode::Tmux`'s shortcut appliers: reads `ShortcutMessage`,
//! `CopyModeMessage`, `TypeMessage`, and `WebviewForwardMessage` from
//! `resolve_key_effects` and applies them as tmux action requests, copy-mode
//! keys, and forwarded pane keystrokes.

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
        shortcuts::{
            CopyModeMessage, ShortcutMessage, ShortcutSet, TypeMessage, WebviewForwardMessage,
        },
        tmux::forward::ForwardPaneKeysRequest,
    },
    ui::copy_mode::EnterCopyModeActionEvent,
};
use bevy::input::keyboard::Key;
use bevy::{ecs::system::SystemParam, prelude::*};
use ozmux_configs::shortcuts::Shortcut;
use ozmux_configs::shortcuts::{
    Modifiers, PaneDirection as CfgPaneDirection, SplitOrientation as CfgSplitOrientation,
};
use ozmux_tmux::{
    ActiveWindow, KeyMods, PaneDirection, SplitDirection, TmuxSession, TmuxWindow,
    bevy_key_to_tmux_name,
};
use std::collections::HashMap;

pub(super) struct ShortcutsTmuxModePlugin;

impl Plugin for ShortcutsTmuxModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_tmux_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<ShortcutMessage>),
                apply_tmux_copy_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<CopyModeMessage>),
                apply_tmux_forward
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_tmux_forward_message())
                    .after(apply_tmux_shortcuts)
                    .after(apply_tmux_copy_mode),
            )
                .in_set(TmuxActiveSet),
        );
    }
}

/// Run condition for `apply_tmux_forward`: true on any frame carrying a key to
/// forward (typed or webview-forwarded). The two never coexist in a frame.
fn on_tmux_forward_message() -> impl SystemCondition<()> {
    on_message::<TypeMessage>.or(on_message::<WebviewForwardMessage>)
}

/// Target-entity lookups for the tmux shortcut actions, bundled to stay
/// within Bevy's system-parameter limit.
#[derive(SystemParam)]
pub(in crate::input) struct ActionTargets<'w, 's> {
    active_window: Query<'w, 's, Entity, With<ActiveWindow>>,
    session: Query<'w, 's, Entity, With<TmuxSession>>,
    windows: Query<'w, 's, (Entity, &'static TmuxWindow)>,
}

/// Applies tmux keyboard shortcuts from `ShortcutMessage`: copy-mode entry,
/// paste (`PasteAction`), detach (`DetachSessionRequest`), and the pane/window
/// action requests. `Quit` / `ReleaseWebviewFocus` are handled upstream in
/// `resolve_key_effects`. Registered in `ShortcutSet::Apply`, gated on
/// `in_state(AppMode::Tmux)` + `on_message::<ShortcutMessage>`, ordered before
/// `apply_tmux_forward`.
pub(in crate::input) fn apply_tmux_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
    targets: ActionTargets,
) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::EnterCopyMode => {
                // NOTE: re-entry guard — re-triggering while already in copy
                // mode would double-insert CopyModeState and re-enter vi mode.
                if let Some(entity) = msg.focused
                    && !msg.in_copy_mode
                {
                    commands.trigger(EnterCopyModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::DetachSession => {
                if let Ok(entity) = targets.session.single() {
                    commands.trigger(DetachSessionRequest { entity });
                }
            }
            action => dispatch_tmux_action(&mut commands, action, msg.focused, &targets),
        }
    }
}

/// Applies matched `[copy-mode]` keys from `CopyModeMessage` on the focused
/// pane. Registered in `ShortcutSet::Apply`, gated on `in_state(AppMode::Tmux)`
/// + `on_message::<CopyModeMessage>`.
pub(in crate::input) fn apply_tmux_copy_mode(
    mut commands: Commands,
    mut copy_mode: MessageReader<CopyModeMessage>,
) {
    for msg in copy_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_copy_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Forwards typed / webview-forwarded keys to the focused pane as one
/// `ForwardPaneKeysRequest` per pane. `TypeMessage` and `WebviewForwardMessage`
/// never coexist in a frame, so at most one reader is non-empty. Runs after the
/// shortcut/copy appliers so their triggers are queued first (parity with the
/// old single-system order). Gated on `on_tmux_forward_message`.
pub(in crate::input) fn apply_tmux_forward(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
    mut webview_forward: MessageReader<WebviewForwardMessage>,
) {
    let mut by_pane: HashMap<Entity, Vec<String>> = HashMap::new();
    for msg in type_keys.read() {
        push_forward_name(
            &mut by_pane,
            msg.focused,
            &msg.logical,
            msg.key_code,
            msg.mods,
        );
    }
    for msg in webview_forward.read() {
        push_forward_name(
            &mut by_pane,
            msg.focused,
            &msg.logical,
            msg.key_code,
            msg.mods,
        );
    }
    for (entity, names) in by_pane {
        commands.trigger(ForwardPaneKeysRequest { entity, names });
    }
}

/// Appends the tmux key name for `(logical, key_code, mods)` to `focused`'s
/// per-pane forward list, when the key maps to a name and a pane is focused.
fn push_forward_name(
    by_pane: &mut HashMap<Entity, Vec<String>>,
    focused: Option<Entity>,
    logical: &Key,
    key_code: KeyCode,
    mods: Modifiers,
) {
    let Some(entity) = focused else {
        return;
    };
    let kmods = KeyMods {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        super_: mods.meta,
    };
    if let Some(name) = bevy_key_to_tmux_name(logical, key_code, kmods) {
        by_pane.entry(entity).or_default().push(name);
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
