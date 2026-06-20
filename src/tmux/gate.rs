//! Per-pane input gating for `AppMode::Ozmux`: every pane is `KeyboardDisabled`
//! (keys pass through to tmux), and `MouseDisabled` whenever a modal owns input,
//! the pane is in copy mode, a webview is interacting, or an interactive inline
//! webview under the cursor claims the press — so `ozma_terminal`'s shared mouse
//! systems yield to the tmux-specific gestures.

use super::pane_hit::tmux_pane_at_phys;
use crate::inline_webview::{InlineWebview, inline_hit_at};
use crate::input::ime::ImeState;
use crate::osc_webview::NonInteractive;
use crate::ozma::AppMode;
use crate::picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{KeyboardDisabled, MouseDisabled, OzmaTerminalInputSet, OzmaTerminalMouseSet};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
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
    metrics: Option<Res<TerminalCellMetricsResource>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    pane_geometry: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<&Children>,
    inline: Query<(&InlineWebview, Has<NonInteractive>)>,
    inline_parents: Query<&ChildOf, With<InlineWebview>>,
    overlay_rects: Query<&TerminalOverlays>,
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
    let window = windows.single().ok();
    let window_focused = window.map(|w| w.focused).unwrap_or(false);
    let modal = picker.open || ime.is_composing() || !window_focused;
    // TODO: a press within `divider_grab_tolerance_px` of a divider can still
    // both resize the pane and start an `ozma_terminal` selection — the
    // divider-band claim is not folded in here yet. Adding it requires the
    // logical-vs-physical divider coordinate space; tracked as a follow-up.
    let claimed_pane = window.and_then(|window| {
        claimed_inline_pane(
            window,
            metrics.as_deref(),
            &pane_geometry,
            &children,
            &inline,
            &inline_parents,
            &overlay_rects,
        )
    });
    for (entity, has_keyboard, has_mouse, in_copy_mode) in panes.iter() {
        if !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        }
        let region_claimed = Some(entity) == claimed_pane;
        let disable_mouse = should_disable_pane_mouse(
            modal,
            in_copy_mode,
            focused_webview.0.is_some(),
            region_claimed,
        );
        if disable_mouse && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable_mouse && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}

fn should_disable_pane_mouse(
    modal: bool,
    in_copy_mode: bool,
    webview_active: bool,
    region_claimed: bool,
) -> bool {
    modal || in_copy_mode || webview_active || region_claimed
}

/// The parent `TmuxPane` entity of the interactive inline webview currently
/// under the cursor, or `None`. Mirrors the press-routing hit-test in the mouse
/// arbiter (`route_tmux_inline_left_click`): `cursor_phys = cursor × scale`,
/// `tmux_pane_at_phys` → `local_phys`, then `inline_hit_at` against the pane's
/// active overlays. `inline_hit_at` already skips `NonInteractive` webviews, so
/// only an interactive one claims. Returns `None` when metrics are absent (no
/// cell pitch to hit-test with), the cursor is off every pane, or no rect is hit.
fn claimed_inline_pane(
    window: &Window,
    metrics: Option<&TerminalCellMetricsResource>,
    pane_geometry: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: &Query<&Children>,
    inline: &Query<(&InlineWebview, Has<NonInteractive>)>,
    inline_parents: &Query<&ChildOf, With<InlineWebview>>,
    overlay_rects: &Query<&TerminalOverlays>,
) -> Option<Entity> {
    let metrics = metrics?;
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position()? * scale;
    let (terminal, _pane_id, local_phys) = tmux_pane_at_phys(pane_geometry, cursor_phys)?;
    let overlays = overlay_rects.get(terminal).ok()?;
    let hit = inline_hit_at(
        children, inline, overlays, terminal, local_phys, cell_w, cell_h, scale,
    )?;
    Some(inline_parents.get(hit.child).ok()?.parent())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_mouse_on_any_guard() {
        assert!(!should_disable_pane_mouse(false, false, false, false));
        assert!(should_disable_pane_mouse(true, false, false, false));
        assert!(should_disable_pane_mouse(false, true, false, false));
        assert!(should_disable_pane_mouse(false, false, true, false));
        assert!(should_disable_pane_mouse(false, false, false, true));
    }
}
