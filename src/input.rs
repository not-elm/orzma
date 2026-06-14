//! Keyboard shortcut handling: dispatcher systems. The shortcut binding
//! table comes from the loaded `OzmuxConfigsResource`; this module owns
//! no chord data.

pub(crate) mod hyperlink;
pub(crate) mod ime;
pub(crate) mod mouse_buttons;
pub(crate) mod mouse_wheel;

use crate::system_set::OzmuxSystems;
use bevy::prelude::*;
use ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozmux_configs::shortcuts::Modifiers;
use ozmux_multiplexer::{
    ActivePane, ActiveSurface, AttachedWorkspace, MultiplexerCommands, WorkspaceMarker,
};

/// Resolves the focused surface's entity via the attached workspace →
/// active pane → active surface chain. The Surface entity *is* its own host,
/// so the active surface entity is returned directly.
pub(crate) fn resolve_focused_terminal(
    mux: &MultiplexerCommands,
    attached_workspace: &Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
) -> Option<Entity> {
    let workspace = attached_workspace.iter().next()?;
    let pane = mux.workspaces_active_pane(workspace)?;
    mux.panes_active_surface(pane)
}

/// Resolves the focused surface's entity using plain read-only `ActivePane` /
/// `ActiveSurface` queries instead of the full `MultiplexerCommands` SystemParam.
/// Systems that mutate `Node`/`Children` cannot also hold `MultiplexerCommands`
/// (its broad `&Node`/`&Children` layout queries alias the mutation — Bevy
/// B0001), so they resolve the focused terminal through this narrow path.
pub(crate) fn resolve_focused_terminal_readonly(
    attached_workspace: &Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    active_panes: &Query<&ActivePane>,
    active_surfaces: &Query<&ActiveSurface>,
) -> Option<Entity> {
    let workspace = attached_workspace.iter().next()?;
    let pane = active_panes.get(workspace).ok()?.0;
    active_surfaces.get(pane).ok().map(|s| s.0)
}

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
        app.configure_sets(
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

/// Resolves the active surface entity for `workspace` and triggers a
/// `TerminalKeyInput` on it. Silently no-ops when the workspace has no
/// active pane/surface yet, or when the target entity has no
/// `TerminalHandle` (e.g. a webview surface) — the `ozma_tty_engine`
/// observer handles that case by also no-op'ing.
pub(crate) fn forward_to_active_terminal(
    commands: &mut Commands,
    mux: &MultiplexerCommands,
    workspace: Entity,
    key: TerminalKey,
    mods: TerminalModifiers,
) {
    let Some(pane) = mux.workspaces_active_pane(workspace) else {
        return;
    };
    let Some(entity) = mux.panes_active_surface(pane) else {
        return;
    };
    commands.trigger(TerminalKeyInput {
        entity,
        key,
        modifiers: mods,
    });
}
