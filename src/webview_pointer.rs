//! Mode-agnostic webview pointer routing: forwards left press/release and
//! pointer motion to the inline CEF child under the cursor, on ANY
//! `OzmaTerminal` surface (a tmux pane or the Default-mode shell). The
//! mode-specific systems (`crate::mode::tmux::mouse::webview`, `crate::mode::default::input`)
//! resolve which surface is under the cursor — multi-pane hit-test for tmux, the
//! single shell for Default — and then delegate the CEF forwarding + focus to
//! the helpers here. Inline webviews are Node/Mesh-free `ChildOf` children
//! (`ozma_webview`), so `bevy_cef`'s native picking cannot reach them; this
//! manual forwarding is the only path that delivers clicks to them.

use crate::surface_geom::phys_to_pane_local;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseScrollUnit};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_terminal::OzmaTerminal;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_webview::{
    NonInteractive, Webview, focused_webview_of, webview_hit_at, webview_local_dip,
};

/// Tracks the CEF child currently pressed (a left press inside an interactive
/// inline rect was forwarded to it) so the matching release routes to the same
/// child even if the pointer drifted off-rect. Shared by both mode pipelines —
/// only one mode is active at a time, so a single in-flight press suffices.
#[derive(Resource, Default)]
pub(crate) struct WebviewPress(pub(crate) Option<Entity>);

/// Queries/resources the webview routing needs, bundled to stay within Bevy's
/// system-parameter limit. Mode-agnostic: the surface-geometry lookup is
/// `With<OzmaTerminal>` (both tmux panes and the Default shell are
/// `OzmaTerminal`), not `TmuxPane`. `focused_webview` / `browsers` are optional
/// so CEF-less tests construct it (state effects still apply).
#[derive(SystemParam)]
pub(crate) struct WebviewRouteParams<'w, 's> {
    pub(crate) focused_webview: Option<ResMut<'w, FocusedWebview>>,
    pub(crate) children: Query<'w, 's, &'static Children>,
    pub(crate) webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    pub(crate) webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
    pub(crate) overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    pub(crate) surface_geo:
        Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<OzmaTerminal>>,
    pub(crate) browsers: Option<NonSend<'w, Browsers>>,
}

/// Routes a left press/release through the webview layer for a resolved
/// `(terminal, local_phys)`, returning `true` when the event was CONSUMED and
/// must NOT reach the host's terminal mouse pipeline.
///
/// A press inside an interactive rect sets `FocusedWebview`, issues the UNGATED
/// `set_focus` BEFORE the gated `send_mouse_click` (CEF drops clicks to a
/// browser with no `focused_frame()`, so the first click would otherwise be
/// swallowed), forwards the press in DIP, and records the in-flight press. A
/// press outside every rect clears an inline `FocusedWebview` and returns
/// `false` (so the press falls through to the terminal). Release forwards the
/// click-up to the recorded child (drift-tolerant) and clears.
#[expect(
    clippy::too_many_arguments,
    reason = "inline routing needs the webview press state, route params, and pointer geometry"
)]
pub(crate) fn route_webview_left_click(
    webview_press: &mut WebviewPress,
    route: &mut WebviewRouteParams,
    terminal: Entity,
    local_phys: Vec2,
    cursor_phys: Vec2,
    button_state: ButtonState,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> bool {
    match button_state {
        ButtonState::Pressed => {
            webview_press.0 = None;
            let hit = route.overlay_rects.get(terminal).ok().and_then(|overlays| {
                webview_hit_at(
                    &route.children,
                    &route.webviews,
                    overlays,
                    terminal,
                    local_phys,
                    cell_w_phys,
                    cell_h_phys,
                    scale,
                )
            });
            let Some(hit) = hit else {
                if let Some(focused) = route.focused_webview.as_deref_mut()
                    && focused
                        .0
                        .is_some_and(|current| route.webview_parents.contains(current))
                {
                    focused.0 = None;
                }
                return false;
            };
            if let Some(focused) = route.focused_webview.as_deref_mut()
                && focused.0 != Some(hit.child)
            {
                focused.0 = Some(hit.child);
            }
            if let Some(browsers) = route.browsers.as_deref() {
                browsers.set_focus(&hit.child, true);
                browsers.send_mouse_click(&hit.child, hit.local_dip, PointerButton::Primary, false);
            }
            webview_press.0 = Some(hit.child);
            true
        }
        ButtonState::Released => {
            let Some(child) = webview_press.0.take() else {
                return false;
            };
            if let Some(browsers) = route.browsers.as_deref()
                && let Some(dip) =
                    webview_release_dip(route, child, cursor_phys, cell_w_phys, cell_h_phys, scale)
            {
                browsers.send_mouse_click(&child, dip, PointerButton::Primary, true);
            }
            true
        }
    }
}

/// Releases an in-flight webview press to CEF (mouse-up at the last cursor) and
/// clears the marker. Called on the suppressed path (modal open / window
/// unfocused) so the focused web page is not left logically pressed with no
/// matching mouse-up. `cursor_phys` is `None` when there is no placeable cursor
/// (off-window): then the press is dropped WITHOUT a CEF mouse-up.
pub(crate) fn release_webview_press(
    webview_press: &mut WebviewPress,
    route: &WebviewRouteParams,
    cursor_phys: Option<Vec2>,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) {
    let Some(child) = webview_press.0.take() else {
        return;
    };
    if let Some(cursor_phys) = cursor_phys
        && let Some(browsers) = route.browsers.as_deref()
        && let Some(dip) =
            webview_release_dip(route, child, cursor_phys, cell_w_phys, cell_h_phys, scale)
    {
        browsers.send_mouse_click(&child, dip, PointerButton::Primary, true);
    }
}

/// Forwards pointer motion over an interactive inline rect of `terminal` to the
/// child's CEF browser (`send_mouse_move`, webview-local DIP), forwarding
/// whatever mouse buttons are held so one call serves both hover and an in-rect
/// drag. Focus-gated inside `bevy_cef`, so motion over an unfocused browser is
/// dropped browser-side. Takes granular query refs (not `WebviewRouteParams`)
/// because the mode move systems read `ButtonInput<MouseButton>`, which the
/// click `SystemParam` does not carry.
#[expect(
    clippy::too_many_arguments,
    reason = "the move forward needs the inline queries, browsers, held buttons, and pointer geometry"
)]
pub(crate) fn forward_webview_move(
    children: &Query<&Children>,
    webviews: &Query<(&Webview, Has<NonInteractive>)>,
    overlay_rects: &Query<&TerminalOverlays>,
    browsers: Option<&Browsers>,
    pressed_buttons: &ButtonInput<MouseButton>,
    terminal: Entity,
    local_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) {
    let Ok(overlays) = overlay_rects.get(terminal) else {
        return;
    };
    let Some(hit) = webview_hit_at(
        children,
        webviews,
        overlays,
        terminal,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale,
    ) else {
        return;
    };
    if let Some(browsers) = browsers {
        browsers.send_mouse_move(
            &hit.child,
            pressed_buttons.get_pressed(),
            hit.local_dip,
            false,
        );
    }
}

/// Returns the topmost `OzmaTerminal` surface whose node contains `cursor_phys`,
/// or `None` when the cursor is over none. "Topmost" is the highest
/// `ComputedNode::stack_index` (Bevy's resolved front-to-back UI order); ties
/// break by `Entity` for determinism. The Default-mode pointer/gate path uses
/// this to pick the single shell (or the frontmost surface) under the cursor;
/// tmux keeps its own multi-pane `tmux_pane_at_phys` resolution.
pub(crate) fn topmost_surface_at<'a>(
    cursor_phys: Vec2,
    candidates: impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)>,
) -> Option<Entity> {
    candidates
        .filter(|&(_, node, transform)| node.contains_point(*transform, cursor_phys))
        .max_by_key(|&(entity, node, _)| (node.stack_index(), entity))
        .map(|(entity, _, _)| entity)
}

/// The inline webview child that should receive a wheel event for a resolved
/// `(terminal, local_phys)`, with the pointer in webview-local DIP — `Some` only
/// when the FOCUSED webview of `terminal` is the interactive rect under the
/// cursor (CEF's `send_mouse_wheel` is focus-gated, so an unfocused rect cannot
/// usefully receive it). `None` cedes the wheel to terminal scrollback.
#[expect(
    clippy::too_many_arguments,
    reason = "wheel targeting needs the focus state, inline queries, and pointer geometry"
)]
pub(crate) fn webview_wheel_target(
    focused_webview: &FocusedWebview,
    webview_parents: &Query<&ChildOf, With<Webview>>,
    children: &Query<&Children>,
    webviews: &Query<(&Webview, Has<NonInteractive>)>,
    overlay_rects: &Query<&TerminalOverlays>,
    terminal: Entity,
    local_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> Option<(Entity, Vec2)> {
    let focused_child = focused_webview_of(Some(focused_webview), webview_parents, Some(terminal))?;
    let overlays = overlay_rects.get(terminal).ok()?;
    let hit = webview_hit_at(
        children,
        webviews,
        overlays,
        terminal,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale,
    )?;
    (hit.child == focused_child).then_some((hit.child, hit.local_dip))
}

/// Converts a Bevy `MouseWheel` (`unit`, `x`, `y`) to the raw CEF wheel delta
/// (no sign flip): `Line` units are scaled by 120 (one notch), `Pixel` units
/// (trackpads / high-resolution wheels) pass through unchanged.
pub(crate) fn webview_wheel_delta(unit: MouseScrollUnit, x: f32, y: f32) -> Vec2 {
    match unit {
        MouseScrollUnit::Line => Vec2::new(x, y) * 120.0,
        MouseScrollUnit::Pixel => Vec2::new(x, y),
    }
}

/// Webview-local DIP for a release on `child`, WITHOUT containment (a pointer
/// that drifted off the rect still produces a release position). `None` when the
/// child/terminal/rect chain is gone.
fn webview_release_dip(
    route: &WebviewRouteParams,
    child: Entity,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> Option<Vec2> {
    let terminal = route.webview_parents.get(child).ok()?.parent();
    let (node, transform) = route.surface_geo.get(terminal).ok()?;
    let local_phys = phys_to_pane_local(node, transform, cursor_phys)?;
    let (view, _) = route.webviews.get(child).ok()?;
    webview_local_dip(
        route.overlay_rects.get(terminal).ok()?,
        view.slot,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale,
    )
}
