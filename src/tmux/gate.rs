//! Per-pane input gating for `AppMode::Ozmux`: every pane is `KeyboardDisabled`
//! (keys pass through to tmux), and `MouseDisabled` whenever a modal owns input,
//! the pane is in copy mode, or a webview is interacting — so `ozma_terminal`'s
//! shared mouse systems yield to the tmux-specific gestures.

use crate::input::ime::ImeState;
use crate::ozma::AppMode;
use crate::picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{KeyboardDisabled, MouseDisabled, OzmaTerminalInputSet, OzmaTerminalMouseSet};
use ozmux_tmux::TmuxPane;

/// Registers the Ozmux-mode per-pane input gate maintainer.
pub(crate) struct GatePlugin;

impl Plugin for GatePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_tmux_input_gates
                .before(OzmaTerminalInputSet)
                .before(OzmaTerminalMouseSet)
                .run_if(in_state(AppMode::Ozmux)),
        );
    }
}

fn maintain_tmux_input_gates(
    mut commands: Commands,
    picker: Res<SessionPicker>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    panes: Query<
        (
            Entity,
            Has<KeyboardDisabled>,
            Has<MouseDisabled>,
            Has<CopyModeState>,
        ),
        With<TmuxPane>,
    >,
) {
    let window_focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let modal = picker.open || ime.is_composing() || !window_focused;
    for (entity, has_keyboard, has_mouse, in_copy_mode) in panes.iter() {
        if !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        }
        let disable_mouse =
            should_disable_pane_mouse(modal, in_copy_mode, focused_webview.0.is_some());
        if disable_mouse && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable_mouse && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}

fn should_disable_pane_mouse(modal: bool, in_copy_mode: bool, webview_active: bool) -> bool {
    modal || in_copy_mode || webview_active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_mouse_on_any_guard() {
        assert!(!should_disable_pane_mouse(false, false, false));
        assert!(should_disable_pane_mouse(true, false, false));
        assert!(should_disable_pane_mouse(false, true, false));
        assert!(should_disable_pane_mouse(false, false, true));
    }
}
