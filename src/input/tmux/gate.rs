//! Per-pane input gating for `AppMode::Tmux`: every pane is `KeyboardDisabled`
//! (keys pass through to tmux), and `MouseDisabled` whenever a modal owns input,
//! the pane is in vi mode, the focused webview belongs to the pane, or
//! an interactive webview under the cursor claims the press — so the
//! `crate::input::mouse` shared systems yield to the tmux-specific gestures.

use super::pane_hit::tmux_pane_at_phys;
use crate::app_mode::AppMode;
use crate::input::InputPhase;
use crate::input::focus::KeyboardDisabled;
use crate::input::focus::MouseDisabled;
use crate::input::ime::ImeState;
use crate::ui::text_prompt::ActiveTextPrompt;
use crate::ui::vi_mode::ViModeState;
use crate::ui::vi_search::ViModePrompt;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use orzma_tmux::TmuxPane;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::prelude::TerminalOverlays;
use orzma_webview::{NonInteractive, Webview, webview_hit_at};

/// Registers the Tmux-mode per-pane input gate maintainer.
pub(super) struct GatePlugin;

impl Plugin for GatePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_tmux_input_gates
                .before(InputPhase::Hover)
                .run_if(in_state(AppMode::Tmux)),
        );
    }
}

fn maintain_tmux_input_gates(
    mut commands: Commands,
    ime: Res<ImeState>,
    vi_mode_prompt: Res<ViModePrompt>,
    active_text_prompt: Res<ActiveTextPrompt>,
    focused_webview: Res<FocusedWebview>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    pane_geometry: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<&Children>,
    webviews: Query<(&Webview, Has<NonInteractive>)>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    overlay_rects: Query<&TerminalOverlays>,
    panes: Query<
        (
            Entity,
            Has<KeyboardDisabled>,
            Has<MouseDisabled>,
            Has<ViModeState>,
        ),
        With<TmuxPane>,
    >,
) {
    let window = windows.single().ok();
    let window_focused = window.map(|w| w.focused).unwrap_or(false);
    let modal = ime.is_composing()
        || !window_focused
        || vi_mode_prompt.open.is_some()
        || active_text_prompt.0.is_some();
    // NOTE: gate only the focused webview's OWNING pane, not all panes — a
    // global `focused_webview.0.is_some()` would kill scroll/selection on every
    // other pane while any webview is focused.
    let focused_webview_pane = focused_webview
        .0
        .and_then(|webview| webview_parents.get(webview).ok())
        .map(|childof| childof.parent());
    // TODO: a press within `divider_grab_tolerance_px` of a divider can still
    // both resize the pane and start a local terminal selection — the
    // divider-band claim is not folded in here yet. Adding it requires the
    // logical-vs-physical divider coordinate space; tracked as a follow-up.
    let claimed_pane = window.and_then(|window| {
        claimed_webview_pane(
            window,
            metrics.as_deref(),
            &pane_geometry,
            &children,
            &webviews,
            &webview_parents,
            &overlay_rects,
        )
    });
    for (entity, has_keyboard, has_mouse, in_vi_mode) in panes.iter() {
        if !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        }
        let region_claimed = Some(entity) == claimed_pane;
        let disable_mouse = should_disable_pane_mouse(
            modal,
            in_vi_mode,
            Some(entity) == focused_webview_pane,
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
    in_vi_mode: bool,
    webview_focused_here: bool,
    region_claimed: bool,
) -> bool {
    modal || in_vi_mode || webview_focused_here || region_claimed
}

/// The parent `TmuxPane` entity of the interactive webview currently
/// under the cursor, or `None`. Mirrors the press-routing hit-test in
/// `tmux_webview_pointer`: `cursor_phys = cursor × scale`,
/// `tmux_pane_at_phys` → `local_phys`, then `webview_hit_at` against the pane's
/// active overlays. `webview_hit_at` already skips `NonInteractive` webviews, so
/// only an interactive one claims. Returns `None` when metrics are absent (no
/// cell pitch to hit-test with), the cursor is off every pane, or no rect is hit.
fn claimed_webview_pane(
    window: &Window,
    metrics: Option<&TerminalCellMetricsResource>,
    pane_geometry: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: &Query<&Children>,
    webviews: &Query<(&Webview, Has<NonInteractive>)>,
    webview_parents: &Query<&ChildOf, With<Webview>>,
    overlay_rects: &Query<&TerminalOverlays>,
) -> Option<Entity> {
    let metrics = metrics?;
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position()? * scale;
    let (terminal, _pane_id, local_phys) = tmux_pane_at_phys(pane_geometry, cursor_phys)?;
    let overlays = overlay_rects.get(terminal).ok()?;
    let hit = webview_hit_at(
        children, webviews, overlays, terminal, local_phys, cell_w, cell_h, scale,
    )?;
    Some(webview_parents.get(hit.child).ok()?.parent())
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
