//! Shared CEF pointer routing helpers for both the default-mode router and the
//! tmux gesture arbiter: forwards left press/release and pointer motion to the
//! inline CEF child under the cursor, on ANY `OrzmaTerminal` surface (a tmux
//! pane or the Default-mode shell). The mode-specific systems
//! (`crate::input::mouse::button::tmux`, `crate::input::mouse::webview::default_mode`) resolve
//! which surface is under the cursor — multi-pane hit-test for tmux, the
//! single shell for Default — and then delegate the CEF forwarding + focus to
//! the helpers here. Inline webviews are Node/Mesh-free `ChildOf` children
//! (`orzma_webview`), so `bevy_cef`'s native picking cannot reach them; this
//! manual forwarding is the only path that delivers clicks to them.

use crate::input::mouse::cell_dims;
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::phys_to_pane_local;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseScrollUnit};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::prelude::TerminalOverlays;
use orzma_webview::{
    NonInteractive, Webview, focused_webview_of, webview_hit_at, webview_local_dip,
};

mod default_mode;

/// Registers the shared webview pointer resource and the per-mode webview routers.
pub(super) struct MouseWebviewPlugin;

impl Plugin for MouseWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WebviewPress>()
            .add_plugins(default_mode::MouseWebviewDefaultModePlugin);
    }
}

/// Tracks the CEF child currently pressed (a left press inside an interactive
/// inline rect was forwarded to it) so the matching release routes to the same
/// child even if the pointer drifted off-rect. Shared by both mode pipelines —
/// only one mode is active at a time, so a single in-flight press suffices.
#[derive(Resource, Default)]
pub(in crate::input::mouse) struct WebviewPress(pub Option<Entity>);

/// Queries/resources the webview routing needs, bundled to stay within Bevy's
/// system-parameter limit. Mode-agnostic: the surface-geometry lookup is
/// `With<OrzmaTerminal>` (both tmux panes and the Default shell are
/// `OrzmaTerminal`), not `TmuxPane`. `focused_webview` / `browsers` are optional
/// so CEF-less tests construct it (state effects still apply).
#[derive(SystemParam)]
pub(in crate::input::mouse) struct WebviewRouteParams<'w, 's> {
    focused_webview: Option<ResMut<'w, FocusedWebview>>,
    children: Query<'w, 's, &'static Children>,
    webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    surface_geo:
        Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<OrzmaTerminal>>,
    browsers: Option<NonSend<'w, Browsers>>,
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
pub(in crate::input::mouse) fn route_webview_left_click(
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
pub(in crate::input::mouse) fn release_webview_press(
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

/// The per-frame pointer geometry both webview pointer pipelines derive from the
/// primary window and cell metrics: the display `scale`, the physical cell pitch
/// `(cell_w, cell_h)`, and the physical-pixel cursor position (`cursor_phys`,
/// `None` when the pointer is off-window).
pub(in crate::input::mouse) struct WebviewPointerFrame {
    pub scale: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cursor_phys: Option<Vec2>,
}

/// Computes the shared `WebviewPointerFrame` for the current frame: `scale` from
/// the window's scale factor, `(cell_w, cell_h)` from the cell metrics, and
/// `cursor_phys` as the window cursor scaled to physical px (`None` off-window).
pub(in crate::input::mouse) fn webview_pointer_frame(
    window: &Window,
    metrics: &TerminalCellMetricsResource,
) -> WebviewPointerFrame {
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_dims(metrics);
    WebviewPointerFrame {
        scale,
        cell_w,
        cell_h,
        cursor_phys: window.cursor_position().map(|c| c * scale),
    }
}

/// The queries, browsers handle, and held buttons `forward_webview_move_at`
/// forwards through to `forward_webview_move`, bundled as one borrowed struct so
/// the wrapper takes a single reference instead of expanded positional args
/// (which would re-trip `clippy::too_many_arguments`). Both mode move systems own
/// these params and borrow them into this bundle each frame.
pub(in crate::input::mouse) struct WebviewMoveDeps<'a> {
    pub children: &'a Query<'a, 'a, &'static Children>,
    pub webviews: &'a Query<'a, 'a, (&'static Webview, Has<NonInteractive>)>,
    pub overlay_rects: &'a Query<'a, 'a, &'static TerminalOverlays>,
    pub browsers: Option<&'a Browsers>,
    pub pressed_buttons: &'a ButtonInput<MouseButton>,
}

/// Resolves the surface under `cursor_phys` via `resolve` (single-shell hit-test
/// for Default mode, multi-pane tmux hit-test for tmux mode) and forwards pointer
/// motion to the inline CEF child there through `forward_webview_move`. A `None`
/// resolution forwards nothing.
pub(in crate::input::mouse) fn forward_webview_move_at(
    deps: &WebviewMoveDeps,
    resolve: impl Fn(Vec2) -> Option<(Entity, Vec2)>,
    cursor_phys: Vec2,
    frame: &WebviewPointerFrame,
) {
    let (terminal, local_phys) = match resolve(cursor_phys) {
        Some(x) => x,
        None => return,
    };
    forward_webview_move(
        deps.children,
        deps.webviews,
        deps.overlay_rects,
        deps.browsers,
        deps.pressed_buttons,
        terminal,
        local_phys,
        frame.cell_w,
        frame.cell_h,
        frame.scale,
    );
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
pub(in crate::input::mouse) fn webview_wheel_target(
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
pub(in crate::input::mouse) fn webview_wheel_delta(unit: MouseScrollUnit, x: f32, y: f32) -> Vec2 {
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
fn forward_webview_move(
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
