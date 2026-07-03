//! Tmux mouse gesture system.
//!
//! Implements a gather→decide→apply pipeline for left-button gestures over tmux panes.
//! `tmux_webview_pointer` gathers raw events and hands unconsumed ones to
//! `TmuxGestureButtons`; `tmux_gesture` reads that buffer and calls the pure
//! deciders (`decide_press`, `decide_release`, `decide_continuation`) which return
//! `TmuxMouseEffect`s; `on_tmux_mouse_effects` (observer in `apply`) applies them
//! by sending tmux control-mode commands.

mod apply;
mod decide;
mod effect;
mod webview;

use super::pane_hit::tmux_pane_at_phys;
use crate::app_mode::TmuxActiveSet;
use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::input::gesture::ClickTracker;
use crate::render::tmux::copy_mode::{CopyModeSnapshot, cell_at_pane};
use crate::render::tmux::{DividerPixelRect, PackedTmuxLayout};
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
pub(crate) use decide::divider_at;
use decide::{
    ContinuationCtx, PressHit, ReleaseCtx, decide_continuation, decide_press, decide_release,
};
use effect::{MultiSelectKind, TmuxMouseEffect, TmuxMouseEffects};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{ActiveWindow, PaneId, TmuxClient, TmuxPane};
use std::time::Duration;
use tmux_control_parser::DividerAxis;
use webview::tmux_webview_pointer;

/// Bevy plugin that registers the tmux mouse gesture system.
pub(super) struct MousePlugin;

impl Plugin for MousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>()
            .init_resource::<TmuxGestureButtons>()
            .add_systems(
                Update,
                tmux_gesture
                    .run_if(pointer_active)
                    .after(tmux_webview_pointer)
                    .in_set(InputPhase::Dispatch)
                    .in_set(TmuxActiveSet),
            )
            .add_plugins((webview::WebviewPointerPlugin, apply::ApplyPlugin));
    }
}

/// Hand-off buffer from `tmux_webview_pointer` to `tmux_gesture`: the
/// frame's left-button events the webview layer did NOT consume.
///
/// `tmux_webview_pointer` clears it at the start of every run and pushes each
/// non-consumed event; `tmux_gesture` drains it via `drain(..)` (keeps the
/// allocation for reuse). It holds only non-consumed `Left` events and never
/// accumulates across frames.
#[derive(Resource, Default)]
pub(super) struct TmuxGestureButtons(pub(super) Vec<MouseButtonInput>);

/// Run condition for the active tmux gesture pipeline: `true` iff a focused
/// primary window exists AND no modal (copy-search prompt) owns input.
///
/// Gates `tmux_gesture` only (which reads the hand-off buffer, not
/// `MouseButtonInput`). `tmux_webview_pointer` is NOT gated by this — it owns the
/// single `MouseButtonInput` reader and runs every frame, computing the same
/// suppressed/active decision in-body. The focused-webview case is NOT a
/// suppressor here — the `tmux_webview_pointer` pre-step owns webview focus.
fn pointer_active(
    windows: Query<&Window, With<PrimaryWindow>>,
    copy_prompt: Res<CopyPrompt>,
) -> bool {
    windows.single().is_ok_and(|window| window.focused) && copy_prompt.open.is_none()
}

/// Bundles the two immutable copy-mode query reads used by `tmux_gesture`.
#[derive(SystemParam)]
struct CopyModeGate<'w, 's> {
    copy_modes: Query<'w, 's, (), With<CopyModeState>>,
    snapshots: Query<'w, 's, &'static CopyModeSnapshot>,
}

/// The current phase of a left-button gesture over a tmux pane.
#[derive(Default, Debug, PartialEq)]
enum GestureState {
    /// No button is held; `tmux_gesture` is waiting for the next press.
    #[default]
    Idle,
    /// Left button is held; `pane`/`pane_id` is the pane that received the press
    /// and `origin_phys` is the physical-pixel cursor position at press time.
    /// Becomes `Selecting` once the pointer drags past `drag_threshold_px`.
    Pressed {
        pane: Entity,
        pane_id: PaneId,
        origin_phys: Vec2,
        click_count: u8,
    },
    /// A double/triple click awaiting its copy-mode snapshot before positioning
    /// the copy cursor and selecting a word/line.
    PendingMultiSelect {
        pane: Entity,
        pane_id: PaneId,
        cell: (u16, u16),
        kind: MultiSelectKind,
    },
    /// Selecting text in a pane via tmux copy-mode (entered on drag-start).
    Selecting {
        pane: Entity,
        pane_id: PaneId,
        anchor: (u16, u16),
        begun: bool,
        last_target: Option<(u16, u16)>,
    },
    /// Dragging a divider to resize its primary pane.
    Resizing {
        divider: DividerPixelRect,
        /// The primary pane's fixed near edge (xoff for vertical, yoff for horizontal), cells.
        near: i32,
        /// Last absolute size (cells) we issued a resize for.
        last_sent: u32,
        /// Whether any `resize-pane` was actually sent (i.e. the pointer
        /// dragged). A press that never drags is a click: on release it falls
        /// back to focusing the pane under the cursor, because the grab zone
        /// overlaps the adjacent pane bodies.
        resized: bool,
    },
}

/// Tracks the current left-button gesture over a tmux pane.
#[derive(Resource, Default)]
struct TmuxMouseGesture {
    state: GestureState,
    click: ClickTracker,
}

/// Physical-pixel cell dimensions derived from `TerminalCellMetricsResource`.
///
/// Returns `(cell_w, cell_h)`: advance and line-height, floored and clamped to at
/// least 1.0 so callers never divide by zero.
pub(super) fn cell_dims(metrics: &TerminalCellMetricsResource) -> (f32, f32) {
    (
        metrics.metrics.advance_phys.floor().max(1.0),
        metrics.metrics.line_height_phys.floor().max(1.0),
    )
}

/// Interprets the non-consumed left-button events handed off by
/// `tmux_webview_pointer` (via `TmuxGestureButtons`) into tmux `select-pane`,
/// `resize-pane`, or selection commands.
///
/// On each `Pressed` event the cursor's physical position is hit-tested: a
/// press within a divider's grab zone (whose primary pane has geometry) enters
/// `Resizing`; otherwise the pane under the cursor is focused (`select-pane`)
/// and the state becomes `Pressed`. While `Pressed`, a pointer that drags past
/// `drag_threshold_px` transitions to `Selecting` when the pane is already in
/// copy mode (drag/selection for a pane NOT in copy mode is owned by
/// the local terminal path). Multi-click (≥2) on a pane in copy mode enters
/// `PendingMultiSelect` to wait for a copy-mode snapshot AND a connected client
/// (it passes whether a `TmuxClient` is present to the decider so a no-client
/// frame stays pending and retries), then selects a word/line via copy-mode
/// commands. Each frame while `Resizing` the pointer's
/// major-axis cell coordinate is mapped to an absolute target size and sent as
/// `resize-pane -x/-y` whenever the target changes. On `Released` from
/// `Selecting` a begun selection is copied to clipboard; from `Resizing` that
/// never dragged, the pane under the cursor is focused as a fallback click.
///
/// Gated by `run_if(pointer_active)`: this system runs only when a focused
/// primary window exists and no modal (copy-search prompt) owns input.
/// The suppressed path — clearing the buffer, resetting the gesture, and
/// releasing or dropping an in-flight inline press — is owned by
/// `tmux_webview_pointer`, which runs every frame upstream of this system.
///
/// The webview pre-step runs UPSTREAM in `tmux_webview_pointer` (ordered before
/// this system via `.after(tmux_webview_pointer)`): a press inside an interactive
/// inline rect focuses + forwards to
/// the child's CEF browser and is consumed (it never reaches this buffer, but its
/// host pane is still `select-pane`d); a press outside every rect drops inline
/// focus and is buffered here as a normal pane gesture.
fn tmux_gesture(
    mut commands: Commands,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: ResMut<TmuxGestureButtons>,
    client: Option<Single<&TmuxClient>>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    packed_q: Query<&PackedTmuxLayout, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OzmuxConfigsResource>>,
    copy_gate: CopyModeGate,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_dims(&metrics);
    let cursor_phys = window.cursor_position().map(|c| c * scale);

    let (grab_tol_logical, drag_threshold_logical, dbl_click_ms, click_drift) = configs
        .as_deref()
        .map(|c| {
            (
                c.mouse.divider_grab_tolerance_px,
                c.mouse.drag_threshold_px,
                c.mouse.double_click_timeout_ms,
                c.mouse.click_drift_px,
            )
        })
        .unwrap_or((4.0, 4.0, 400, 8.0));
    let drag_threshold_phys = drag_threshold_logical * scale;
    let dbl_click = (Duration::from_millis(dbl_click_ms as u64), click_drift);

    let packed_dividers: &[DividerPixelRect] = packed_q
        .single()
        .map(|p| p.dividers.as_slice())
        .unwrap_or(&[]);

    let mut effects: Vec<TmuxMouseEffect> = Vec::new();

    for ev in buttons.0.drain(..) {
        match ev.state {
            ButtonState::Pressed => {
                let Some(cursor_phys) = cursor_phys else {
                    continue;
                };
                let Some(hit) = press_hit(
                    &panes,
                    packed_dividers,
                    cursor_phys,
                    scale,
                    grab_tol_logical,
                ) else {
                    continue;
                };
                let TmuxMouseGesture { state, click, .. } = &mut *gesture;
                effects.extend(decide_press(state, click, hit, time.elapsed(), dbl_click));
            }
            ButtonState::Released => {
                let ctx = release_ctx(
                    &gesture.state,
                    &panes,
                    &copy_gate,
                    cursor_phys,
                    cell_w,
                    cell_h,
                );
                effects.extend(decide_release(&mut gesture.state, ctx));
            }
        }
    }

    let ctx = continuation_ctx(
        &gesture.state,
        &panes,
        &copy_gate,
        cursor_phys,
        drag_threshold_phys,
        cell_w,
        cell_h,
        client.is_some(),
    );
    effects.extend(decide_continuation(&mut gesture.state, ctx));

    if !effects.is_empty() {
        // NOTE: Entity::PLACEHOLDER is correct because on_tmux_mouse_effects is a global observer
        // (app.add_observer) that never reads ev.entity() — every effect variant carries its own
        // PaneId. Must be revisited if the observer becomes entity-scoped or starts reading ev.entity().
        commands.trigger(TmuxMouseEffects {
            entity: Entity::PLACEHOLDER,
            effects,
        });
    }
}

/// Hit-tests a left press at `cursor_phys` into a `PressHit`: a divider grab
/// (resolved to its primary pane's near edge + current size) takes priority over
/// a pane-body focus. A divider whose primary pane has no projected geometry yet
/// falls through to a pane focus rather than a bogus (0) resize baseline; `None`
/// means the press landed on neither.
fn press_hit(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    dividers: &[DividerPixelRect],
    cursor_phys: Vec2,
    scale: f32,
    grab_tol_logical: f32,
) -> Option<PressHit> {
    let cursor_logical = cursor_phys / scale;
    let resize = divider_at(dividers, cursor_logical, grab_tol_logical).and_then(|d| {
        panes
            .iter()
            .find(|(_, p, _, _)| p.id == d.primary)
            .map(|(_, p, _, _)| match d.axis {
                DividerAxis::Vertical => (d, p.dims.xoff, p.dims.width),
                DividerAxis::Horizontal => (d, p.dims.yoff, p.dims.height),
            })
    });
    if let Some((divider, near, last_sent)) = resize {
        return Some(PressHit::Divider {
            divider,
            near,
            last_sent,
        });
    }
    let (pane, pane_id, _) = tmux_pane_at_phys(panes, cursor_phys)?;
    Some(PressHit::Pane {
        pane,
        pane_id,
        origin_phys: cursor_phys,
        cursor_logical,
    })
}

/// Resolves the `ReleaseCtx` for the gesture's current (pre-release) state:
/// copy-mode + a `Pressed` multi-click's origin cell, and the pane under the
/// cursor for a `Resizing`-click focus fallback.
fn release_ctx(
    state: &GestureState,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    copy_gate: &CopyModeGate,
    cursor_phys: Option<Vec2>,
    cell_w: f32,
    cell_h: f32,
) -> ReleaseCtx {
    match *state {
        GestureState::Pressed {
            pane, origin_phys, ..
        } => {
            let copy_mode = copy_gate.copy_modes.get(pane).is_ok();
            let multi_cell = panes.get(pane).ok().and_then(|(_, p, node, transform)| {
                let cols = p.dims.width as u16;
                let rows = p.dims.height as u16;
                cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows)
            });
            ReleaseCtx {
                copy_mode,
                multi_cell,
                pane_under: None,
            }
        }
        GestureState::Resizing { .. } => ReleaseCtx {
            copy_mode: false,
            multi_cell: None,
            pane_under: cursor_phys.and_then(|c| tmux_pane_at_phys(panes, c).map(|(_, id, _)| id)),
        },
        _ => ReleaseCtx {
            copy_mode: false,
            multi_cell: None,
            pane_under: None,
        },
    }
}

/// Resolves the per-frame `ContinuationCtx` for the gesture's current state,
/// reading only the inputs the active arm needs (cursor + copy-mode + origin
/// anchor for `Pressed`; snapshot + live cell for `Selecting`; snapshot +
/// client presence for `PendingMultiSelect`; pointer cell for `Resizing`).
fn continuation_ctx(
    state: &GestureState,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    copy_gate: &CopyModeGate,
    cursor_phys: Option<Vec2>,
    drag_threshold_phys: f32,
    cell_w: f32,
    cell_h: f32,
    client_present: bool,
) -> ContinuationCtx {
    let mut ctx = ContinuationCtx {
        pane_alive: false,
        cursor_phys,
        drag_threshold_phys,
        copy_mode: false,
        anchor_cell: None,
        snapshot_cursor: None,
        selecting_cell: None,
        resize_pointer_cell: None,
        client_present,
    };
    match *state {
        GestureState::Pressed {
            pane, origin_phys, ..
        } => {
            ctx.copy_mode = copy_gate.copy_modes.get(pane).is_ok();
            if let Ok((_, p, node, transform)) = panes.get(pane) {
                ctx.pane_alive = true;
                let cols = p.dims.width as u16;
                let rows = p.dims.height as u16;
                ctx.anchor_cell =
                    cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows);
            }
        }
        GestureState::Selecting { pane, .. } => {
            if let Ok((_, p, node, transform)) = panes.get(pane) {
                ctx.pane_alive = true;
                ctx.snapshot_cursor = copy_gate
                    .snapshots
                    .get(pane)
                    .map(|s| (s.0.cursor_x, s.0.cursor_y))
                    .ok();
                if let Some(cursor_phys) = cursor_phys {
                    let cols = p.dims.width as u16;
                    let rows = p.dims.height as u16;
                    ctx.selecting_cell =
                        cell_at_pane(node, transform, cursor_phys, cell_w, cell_h, cols, rows);
                }
            }
        }
        GestureState::PendingMultiSelect { pane, .. } => {
            if panes.get(pane).is_ok() {
                ctx.pane_alive = true;
                ctx.snapshot_cursor = copy_gate
                    .snapshots
                    .get(pane)
                    .map(|s| (s.0.cursor_x, s.0.cursor_y))
                    .ok();
            }
        }
        GestureState::Resizing { divider, .. } => {
            ctx.pane_alive = true;
            ctx.resize_pointer_cell = cursor_phys.map(|c| match divider.axis {
                DividerAxis::Vertical => (c.x / cell_w).floor() as i32,
                DividerAxis::Horizontal => (c.y / cell_h).floor() as i32,
            });
        }
        GestureState::Idle => {}
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::tmux::pane_hit::tmux_pane_at_phys;
    use crate::webview_pointer::WebviewPress;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use bevy_cef::prelude::FocusedWebview;
    use ozma_tty_renderer::CellMetrics;
    use ozma_tty_renderer::prelude::TerminalOverlays;
    use ozma_webview::{NonInteractive, Webview, webview_hit_at};
    use ozmux_tmux::CopyModeQueries;

    #[test]
    fn gesture_state_default_is_idle() {
        assert_eq!(GestureState::default(), GestureState::Idle);
    }

    fn test_metrics() -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        }
    }

    fn set_modal_open(app: &mut App, open: bool) {
        use crate::ui::copy_search::CopyPromptState;
        use ozmux_tmux::{PaneId, PromptKind};

        app.world_mut().resource_mut::<CopyPrompt>().open = open.then(|| CopyPromptState {
            kind: PromptKind::SearchForward,
            pane: PaneId(0),
            text: String::new(),
        });
    }

    #[test]
    fn left_press_without_cursor_stays_idle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<TmuxGestureButtons>();
        app.init_resource::<WebviewPress>();
        app.init_resource::<CopyModeQueries>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(
            Update,
            (
                webview::tmux_webview_pointer,
                tmux_gesture.run_if(pointer_active),
            )
                .chain(),
        );
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: bevy::input::mouse::MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle
        );
    }

    fn make_gesture_webview_app() -> (App, Entity, Entity) {
        use bevy::window::WindowResolution;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<TmuxGestureButtons>();
        app.init_resource::<WebviewPress>();
        app.init_resource::<CopyModeQueries>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(
            Update,
            (
                webview::tmux_webview_pointer,
                tmux_gesture.run_if(pointer_active),
            )
                .chain(),
        );
        app.add_plugins(apply::ApplyPlugin);

        // Pane host node at window center (400, 300), size 800x600 → top-left
        // at (0, 0). Rect rows 2..12, cols 3..43 → phys y 32..192, x 24..344 at
        // 8x16 px.
        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = IVec4::new(2, 3, 10, 40);
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 100,
                        height: 37,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
                overlays,
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                Webview {
                    view_id: "webview".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        (app, pane, child)
    }

    fn set_cursor(app: &mut App, phys: Vec2) {
        use bevy::math::DVec2;

        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(win)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(phys.x as f64, phys.y as f64)));
    }

    fn write_button(app: &mut App, button: bevy::input::mouse::MouseButton, state: ButtonState) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    #[test]
    fn webview_press_focuses_child_and_consumes() {
        let (mut app, _pane, child) = make_gesture_webview_app();
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "a press inside an interactive inline rect must focus that child"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a consumed inline press must NOT arm a Pressed/Selecting gesture"
        );
    }

    #[test]
    fn move_resolves_inline_child_over_rect() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _pane, child) = make_gesture_webview_app();
        let hit = app
            .world_mut()
            .run_system_once(
                move |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                      children: Query<&Children>,
                      webviews: Query<(&Webview, Has<NonInteractive>)>,
                      overlays: Query<&TerminalOverlays>| {
                    let (terminal, _pane_id, local) =
                        tmux_pane_at_phys(&panes, Vec2::new(40.0, 48.0)).unwrap();
                    webview_hit_at(
                        &children,
                        &webviews,
                        overlays.get(terminal).unwrap(),
                        terminal,
                        local,
                        8.0,
                        16.0,
                        1.0,
                    )
                    .map(|h| h.child)
                },
            )
            .unwrap();
        assert_eq!(
            hit,
            Some(child),
            "pointer over the rect must resolve the inline child"
        );
    }

    #[test]
    fn move_resolves_nothing_off_rect() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _pane, _child) = make_gesture_webview_app();
        let hit = app
            .world_mut()
            .run_system_once(
                |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                 children: Query<&Children>,
                 webviews: Query<(&Webview, Has<NonInteractive>)>,
                 overlays: Query<&TerminalOverlays>| {
                    let (terminal, _pane_id, local) =
                        tmux_pane_at_phys(&panes, Vec2::new(400.0, 400.0)).unwrap();
                    webview_hit_at(
                        &children,
                        &webviews,
                        overlays.get(terminal).unwrap(),
                        terminal,
                        local,
                        8.0,
                        16.0,
                        1.0,
                    )
                    .map(|h| h.child)
                },
            )
            .unwrap();
        assert_eq!(
            hit, None,
            "pointer over terminal text must resolve no inline child"
        );
    }

    #[test]
    fn inline_off_rect_press_releases_focus_and_falls_through() {
        let (mut app, pane, child) = make_gesture_webview_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "an off-rect press must release inline focus"
        );
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "an off-rect press must fall through to the normal pane gesture"
        );
    }

    #[test]
    fn modal_open_suppresses_webview_routing_and_gesture() {
        let (mut app, _pane, _child) = make_gesture_webview_app();
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "a press while a modal is open must not route to the webview layer"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a press while a modal is open must not arm a gesture"
        );
        assert!(
            app.world().resource::<TmuxGestureButtons>().0.is_empty(),
            "a gated frame must buffer no events"
        );
    }

    #[test]
    fn closing_modal_resumes_pointer_handling() {
        let (mut app, pane, _child) = make_gesture_webview_app();
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "while the modal is open the suppressed drain keeps the gesture Idle"
        );

        set_modal_open(&mut app, false);
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "closing the modal must let the active path arm the gesture again"
        );
    }

    #[test]
    fn suppressed_frame_press_does_not_resurface_when_pointer_reactivates() {
        let (mut app, _pane, _child) = make_gesture_webview_app();
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "the suppressed frame must not arm a gesture"
        );

        set_modal_open(&mut app, false);
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a press written during the suppressed frame must not resurface and arm a \
             gesture once the pointer pipeline reactivates"
        );
        assert!(
            app.world().resource::<TmuxGestureButtons>().0.is_empty(),
            "no stale event may reach the hand-off buffer on the reactivated frame"
        );
    }

    #[test]
    fn suppressed_frame_releases_in_flight_webview_press() {
        let (mut app, _pane, child) = make_gesture_webview_app();
        app.world_mut().resource_mut::<WebviewPress>().0 = Some(child);
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            None,
            "a window-exists-but-suppressed frame must release the in-flight inline press"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "the suppressed drain resets the gesture to Idle"
        );
    }

    #[test]
    fn non_consumed_press_is_handed_off_and_buffer_drained() {
        let (mut app, pane, _child) = make_gesture_webview_app();
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "a non-consumed press must reach tmux_gesture through the buffer and arm the gesture"
        );
        assert!(
            app.world().resource::<TmuxGestureButtons>().0.is_empty(),
            "tmux_gesture must drain the hand-off buffer (invariant 7: no cross-frame accumulation)"
        );
    }
}
