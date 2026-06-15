//! Bevy system that translates mouse-wheel input into either an inline
//! webview page scroll, host scrollback adjustments, or PTY-bound mouse /
//! arrow-key bytes via the pure router in
//! `ozma_tty_engine::wheel::WheelAction::route`.
//!
//! Inline-webview fork (spec §7): when a `FocusedWebview` holds an inline
//! child of the active surface AND the pointer currently sits over THAT
//! child's rect, each `MouseWheel` event is forwarded RAW to CEF
//! (`MouseScrollUnit::Line → delta × 120`, `Pixel` as-is, NO `-ev.y` flip)
//! and skipped before the terminal accumulator — routing the gesture through
//! the terminal's notch quantization would break the scroll magnitude and
//! lose smooth scrolling. Wheel follows FOCUS: an unfocused (or tab-webview)
//! inline rect under the pointer still scrolls the terminal.
//!
//! Per-frame flow:
//!
//! 1. Read `MessageReader<MouseWheel>`. Inline-routed events are forwarded
//!    and dropped here. For terminal `Line` units, accumulate `y` into
//!    `residual_y`. For `Pixel` units, divide by the cell height and
//!    accumulate.
//! 2. Reset the residual when the sign flips or the focused entity
//!    changes — both signals indicate the previous accumulation is
//!    stale.
//! 3. Truncate the residual to an integer `notches` count; the
//!    fractional remainder carries to the next frame.
//! 4. Resolve the active workspace's focused pane → entity via
//!    `resolve_focused_terminal`. If copy mode is active, skip — the
//!    copy mode handler owns input semantics there.
//! 5. Resolve the cursor cell within the focused pane (or fall back
//!    to `(1, 1)`).
//! 6. Build `WheelModifiers` from `ButtonInput<KeyCode>` using the
//!    config's `fine_modifier` to set `mods.fine`.
//! 7. Call `WheelAction::route` once; dispatch the returned
//!    `WheelAction` to the focused entity's `TerminalHandle`.

use bevy::ecs::system::SystemParam;
use bevy::input::ButtonInput;
use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_tty_engine::{
    CellCoord, Coalescer, PtyHandle, TerminalHandle, WheelAction, WheelConfig, WheelModifiers,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozmux_configs::mouse::FineModifier;

use crate::configs::OzmuxConfigsResource;
use crate::inline_webview::{InlineWebview, focused_inline_of};
use crate::input::InputPhase;
use crate::input::mouse_buttons::phys_to_terminal_local;
use crate::osc_webview::NonInteractive;
use crate::ui::Slotted;
use crate::ui::copy_mode::CopyModeState;

/// Per-frame accumulator that carries fractional Pixel deltas across
/// frames and tracks the entity the residual was earned on (so a focus
/// change clears stale momentum).
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_y: f32,
    last_entity: Option<Entity>,
}

/// The system params the inline-webview fork of `dispatch_mouse_wheel` needs,
/// bundled so the system stays within Bevy's system-parameter limit.
/// `focused_webview` and `browsers` are optional so CEF-less tests construct
/// the system.
#[derive(SystemParam)]
struct InlineWheelParams<'w, 's> {
    focused_webview: Option<Res<'w, FocusedWebview>>,
    inline_parents: Query<'w, 's, &'static ChildOf, With<InlineWebview>>,
    hosts: Query<
        'w,
        's,
        (Entity, &'static ComputedNode, &'static UiGlobalTransform),
        (With<ozmux_multiplexer::SurfaceMarker>, With<Slotted>),
    >,
    children: Query<'w, 's, &'static Children>,
    inline: Query<'w, 's, (&'static InlineWebview, Has<NonInteractive>)>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Bevy Plugin that registers `WheelAccumulator` and the
/// `dispatch_mouse_wheel` system against the `Update` schedule.
pub(crate) struct MouseWheelInputPlugin;

impl Plugin for MouseWheelInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WheelAccumulator>()
            .add_systems(Update, dispatch_mouse_wheel.in_set(InputPhase::Dispatch));
    }
}

fn dispatch_mouse_wheel(
    mut wheel_msgs: MessageReader<MouseWheel>,
    mut accumulator: ResMut<WheelAccumulator>,
    mut handles: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer)>,
    inline_wheel: InlineWheelParams,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<OzmuxConfigsResource>,
    mux: ozmux_multiplexer::MultiplexerCommands,
    copy_modes: Query<(), With<CopyModeState>>,
    attached_workspace: Query<
        Entity,
        (
            With<ozmux_multiplexer::WorkspaceMarker>,
            With<ozmux_multiplexer::AttachedWorkspace>,
        ),
    >,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    // Cell pitch comes from the font-derived TerminalCellMetricsResource
    // (physical px, DPR-adjusted by `update_terminal_material`). MouseWheel
    // `Pixel` events and `cursor_position()` both report logical px, so we
    // divide the phys metrics by DPR to compare apples-to-apples here.
    let dpr = windows
        .iter()
        .next()
        .map(|w| w.scale_factor())
        .unwrap_or(1.0);
    let cell_w_logical = (metrics.metrics.advance_phys.floor() / dpr).max(1.0);
    let cell_h_logical = (metrics.metrics.line_height_phys.floor() / dpr).max(1.0);
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    let active_surface = super::resolve_focused_terminal(&mux, &attached_workspace);
    let inline_target = resolve_inline_wheel_target(
        &inline_wheel,
        active_surface,
        cursor_phys(&windows, dpr),
        cell_w_phys,
        cell_h_phys,
        dpr,
    );

    let Some(delta_y) = aggregate_wheel_delta(
        &mut wheel_msgs,
        inline_target,
        inline_wheel.browsers.as_deref(),
        cell_h_logical,
    ) else {
        return;
    };
    let Some(entity) = active_surface else {
        return;
    };
    if copy_modes.get(entity).is_ok() {
        return;
    }
    let mouse_cfg = &configs.mouse;
    let Some(notches) =
        consume_notches(&mut accumulator, entity, delta_y, mouse_cfg.cells_per_notch)
    else {
        return;
    };
    let mods = build_wheel_modifiers(&keys, mouse_cfg.fine_modifier);
    let cursor = cursor_cell(&windows, cell_w_logical, cell_h_logical);
    let Ok((mut handle, mut pty, mut coalescer)) = handles.get_mut(entity) else {
        return;
    };
    let action = WheelAction::route(
        handle.current_modes(),
        notches,
        cursor,
        mods,
        &wheel_config(mouse_cfg),
    );
    apply_wheel_action(action, &mut handle, &mut pty, &mut coalescer, entity);
}

/// A focused inline webview child claiming the wheel: the child entity to
/// forward `send_mouse_wheel` to, and the pointer position in that child's
/// webview-local DIP (the coordinate CEF wheel events expect).
#[derive(Debug, Clone, Copy, PartialEq)]
struct InlineWheelTarget {
    child: Entity,
    position_dip: Vec2,
}

/// Aggregates a frame's `MouseWheel` events into a single signed terminal
/// cell-delta, forking inline-routed events out FIRST (spec §7). Returns
/// `None` when no terminal-bound events arrived (every event went inline, or
/// the frame was empty) — the caller then skips the whole terminal path.
///
/// When `inline_target` is `Some`, every event is forwarded RAW to that
/// child's CEF browser via `send_mouse_wheel` (`Line → ×120`, `Pixel` as-is,
/// no `-ev.y` flip) and dropped before terminal accumulation, so the gesture
/// never reaches the notch quantizer.
///
/// For terminal-bound events: winit reports positive `y` when natural
/// scrolling moves the viewport content downward (revealing older lines
/// above); our router uses the opposite convention (`notches < 0` = up /
/// older), so the sign is flipped here.
fn aggregate_wheel_delta(
    events: &mut MessageReader<MouseWheel>,
    inline_target: Option<InlineWheelTarget>,
    browsers: Option<&Browsers>,
    cell_h_logical: f32,
) -> Option<f32> {
    let mut delta_y = 0.0f32;
    let mut had_terminal_input = false;
    for ev in events.read() {
        if let Some(target) = inline_target {
            if let Some(browsers) = browsers {
                browsers.send_mouse_wheel(
                    &target.child,
                    target.position_dip,
                    inline_wheel_delta(ev.unit, ev.x, ev.y),
                );
            }
            continue;
        }
        had_terminal_input = true;
        let cells = match ev.unit {
            MouseScrollUnit::Line => -ev.y,
            MouseScrollUnit::Pixel => -ev.y / cell_h_logical,
        };
        delta_y += cells;
    }
    had_terminal_input.then_some(delta_y)
}

/// Converts one `MouseWheel` event into the RAW CEF wheel delta (spec §7,
/// matching bevy_cef's UI input path): `MouseScrollUnit::Line → (x, y) × 120`
/// (one line = 120 px, the Win32 `WHEEL_DELTA` convention CEF expects),
/// `MouseScrollUnit::Pixel → (x, y)` unchanged. The sign is NOT flipped — the
/// terminal path's `-ev.y` inversion does not apply to the webview path.
fn inline_wheel_delta(unit: MouseScrollUnit, x: f32, y: f32) -> Vec2 {
    match unit {
        MouseScrollUnit::Line => Vec2::new(x, y) * 120.0,
        MouseScrollUnit::Pixel => Vec2::new(x, y),
    }
}

/// The window cursor position in physical px, or `None` when no cursor
/// position is available (cursor outside the window, no primary window).
fn cursor_phys(windows: &Query<&Window, With<PrimaryWindow>>, scale_factor: f32) -> Option<Vec2> {
    windows
        .iter()
        .next()
        .and_then(Window::cursor_position)
        .map(|pos| pos * scale_factor)
}

/// Resolves the inline webview that should receive this frame's wheel events,
/// or `None` (the terminal scrollback path runs instead). Returns `Some` only
/// when a `FocusedWebview` holds an inline child of `active_surface`
/// (`focused_inline_of`) AND the pointer currently sits over THAT child's
/// active overlay rect — wheel follows FOCUS, not mere hover (spec §7).
fn resolve_inline_wheel_target(
    params: &InlineWheelParams,
    active_surface: Option<Entity>,
    cursor_phys: Option<Vec2>,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Option<InlineWheelTarget> {
    let focused_child = focused_inline_of(
        params.focused_webview.as_deref(),
        &params.inline_parents,
        active_surface,
    )?;
    let terminal = params.inline_parents.get(focused_child).ok()?.parent();
    let cursor_phys = cursor_phys?;
    let (_, node, transform) = params.hosts.get(terminal).ok()?;
    if !node.contains_point(*transform, cursor_phys) {
        return None;
    }
    let local_phys = phys_to_terminal_local(node, transform, cursor_phys)?;
    let overlays = params.overlay_rects.get(terminal).ok()?;
    let hit = crate::inline_webview::inline_hit_at(
        &params.children,
        &params.inline,
        overlays,
        terminal,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale_factor,
    )?;
    (hit.child == focused_child).then_some(InlineWheelTarget {
        child: hit.child,
        position_dip: hit.local_dip,
    })
}

/// Updates the per-frame accumulator and returns the integer notch
/// count to dispatch, or `None` when the residual hasn't crossed the
/// notch threshold yet.
///
/// Resets the residual on focus change or sign flip — both signal
/// that any prior momentum is stale.
fn consume_notches(
    accumulator: &mut WheelAccumulator,
    entity: Entity,
    delta_y: f32,
    cells_per_notch: f32,
) -> Option<i32> {
    if accumulator.last_entity != Some(entity) {
        accumulator.residual_y = 0.0;
        accumulator.last_entity = Some(entity);
    } else if accumulator.residual_y.signum() != delta_y.signum() && accumulator.residual_y != 0.0 {
        accumulator.residual_y = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    accumulator.residual_y += delta_y;
    let notches = (accumulator.residual_y / threshold).trunc() as i32;
    if notches == 0 {
        return None;
    }
    accumulator.residual_y -= notches as f32 * threshold;
    Some(notches)
}

/// Captures the current keyboard modifier state, resolving
/// `mods.fine` against the configured `fine_modifier`.
fn build_wheel_modifiers(
    keys: &ButtonInput<KeyCode>,
    fine_modifier: FineModifier,
) -> WheelModifiers {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let fine = match fine_modifier {
        FineModifier::Shift => shift,
        FineModifier::Ctrl => ctrl,
        FineModifier::Alt => alt,
        FineModifier::None => false,
    };
    WheelModifiers {
        shift,
        ctrl,
        alt,
        fine,
    }
}

/// Translates the window cursor position into a 1-indexed cell
/// coordinate. Falls back to `(1, 1)` when no cursor position is
/// available (cursor outside the window, no primary window matched).
fn cursor_cell(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cell_w_logical: f32,
    cell_h_logical: f32,
) -> CellCoord {
    windows
        .iter()
        .next()
        .and_then(|w| w.cursor_position())
        .map(|pos| CellCoord {
            col: ((pos.x / cell_w_logical) as u32).saturating_add(1).max(1),
            row: ((pos.y / cell_h_logical) as u32).saturating_add(1).max(1),
        })
        .unwrap_or(CellCoord { col: 1, row: 1 })
}

/// Projects the runtime `MouseConfig` onto the router's `WheelConfig`
/// (the per-call subset the pure router needs).
fn wheel_config(cfg: &ozmux_configs::mouse::MouseConfig) -> WheelConfig {
    WheelConfig {
        lines_per_notch: cfg.lines_per_notch,
        fine_lines: cfg.fine_lines,
        max_protocol_events_per_frame: cfg.max_protocol_events_per_frame,
    }
}

/// Applies a router-decided `WheelAction` to the focused terminal —
/// either scrolls the viewport or writes pre-encoded bytes to the
/// PTY (snapping to live tail first for the write path).
fn apply_wheel_action(
    action: WheelAction,
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    entity: Entity,
) {
    match action {
        WheelAction::Noop => {}
        WheelAction::ScrollViewport(delta) => {
            handle.scroll(coalescer, delta);
        }
        WheelAction::WriteToPty(bytes) => {
            if !handle.is_at_bottom() {
                handle.scroll_to_bottom(coalescer);
            }
            if let Err(e) = handle.write(pty, &bytes) {
                tracing::warn!(?e, ?entity, "mouse wheel write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline_webview::InlineWebview;
    use crate::ui::Slotted;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::DVec2;
    use bevy::window::WindowResolution;
    use ozma_tty_renderer::prelude::TerminalOverlays;
    use ozmux_multiplexer::SurfaceMarker;

    #[test]
    fn wheel_accumulator_default_is_zero() {
        let acc = WheelAccumulator::default();
        assert_eq!(acc.residual_y, 0.0);
        assert!(acc.last_entity.is_none());
    }

    #[test]
    fn inline_wheel_delta_scales_line_units_by_120_with_raw_sign() {
        assert_eq!(
            inline_wheel_delta(MouseScrollUnit::Line, 0.0, 1.0),
            Vec2::new(0.0, 120.0),
            "one line down must produce +120 on y (raw sign, no flip)"
        );
        assert_eq!(
            inline_wheel_delta(MouseScrollUnit::Line, -2.0, 0.0),
            Vec2::new(-240.0, 0.0),
            "horizontal lines scale by 120 too, sign preserved"
        );
    }

    #[test]
    fn inline_wheel_delta_passes_pixel_units_through_unchanged() {
        assert_eq!(
            inline_wheel_delta(MouseScrollUnit::Pixel, 3.0, 7.0),
            Vec2::new(3.0, 7.0),
            "pixel units pass through raw, no 120 scale, no flip"
        );
        assert_eq!(
            inline_wheel_delta(MouseScrollUnit::Pixel, 0.0, -7.0),
            Vec2::new(0.0, -7.0),
        );
    }

    fn write_wheel(app: &mut App, unit: MouseScrollUnit, x: f32, y: f32) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(MouseWheel {
                unit,
                x,
                y,
                window: Entity::PLACEHOLDER,
            });
    }

    fn drain_terminal_delta(app: &mut App, target: Option<InlineWheelTarget>) -> Option<f32> {
        app.world_mut()
            .run_system_once(
                move |mut reader: MessageReader<MouseWheel>,
                      browsers: Option<NonSend<Browsers>>| {
                    aggregate_wheel_delta(&mut reader, target, browsers.as_deref(), 16.0)
                },
            )
            .unwrap()
    }

    #[test]
    fn aggregate_skips_terminal_path_when_inline_target_present() {
        let mut app = App::new();
        app.add_message::<MouseWheel>();
        write_wheel(&mut app, MouseScrollUnit::Line, 0.0, 1.0);
        write_wheel(&mut app, MouseScrollUnit::Line, 0.0, 1.0);

        let target = Some(InlineWheelTarget {
            child: Entity::PLACEHOLDER,
            position_dip: Vec2::new(10.0, 20.0),
        });
        assert_eq!(
            drain_terminal_delta(&mut app, target),
            None,
            "with an inline target every event is forked to CEF; the terminal accumulator must see nothing"
        );
    }

    #[test]
    fn aggregate_feeds_terminal_when_no_inline_target() {
        let mut app = App::new();
        app.add_message::<MouseWheel>();
        write_wheel(&mut app, MouseScrollUnit::Line, 0.0, 1.0);

        assert_eq!(
            drain_terminal_delta(&mut app, None),
            Some(-1.0),
            "without an inline target the terminal path runs with the -ev.y sign flip"
        );
    }

    fn make_wheel_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ozmux_multiplexer::MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.init_resource::<Assets<Image>>();

        let workspace = app
            .world_mut()
            .run_system_once(|mut mux: ozmux_multiplexer::MultiplexerCommands| {
                mux.create_workspace(Some("t".into()))
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(workspace.workspace)
            .insert(ozmux_multiplexer::AttachedWorkspace);
        let surface = workspace.surface;

        // The Surface is its own host: give it the layout components
        // `resolve_inline_wheel_target` hit-tests against. Node at window
        // center (400, 300), size 800x600 → top-left at (0, 0).
        app.world_mut().entity_mut(surface).insert((
            SurfaceMarker,
            Slotted,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 300.0),
            TerminalOverlays::default(),
        ));

        // Rect rows 2..12, cols 3..43 → phys y 32..192, x 24..344 at 8x16 px.
        let child = app
            .world_mut()
            .spawn((
                ChildOf(surface),
                InlineWebview {
                    view_id: "inline".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();
        app.world_mut()
            .get_mut::<TerminalOverlays>(surface)
            .unwrap()
            .rects[0] = IVec4::new(2, 3, 10, 40);

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        (app, surface, child)
    }

    fn set_cursor(app: &mut App, phys: Vec2) {
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

    fn run_resolve_target(app: &mut App) -> Option<InlineWheelTarget> {
        app.world_mut()
            .run_system_once(
                |params: InlineWheelParams,
                 mux: ozmux_multiplexer::MultiplexerCommands,
                 attached: Query<
                    Entity,
                    (
                        With<ozmux_multiplexer::WorkspaceMarker>,
                        With<ozmux_multiplexer::AttachedWorkspace>,
                    ),
                >,
                 windows: Query<&Window, With<PrimaryWindow>>| {
                    let active = super::super::resolve_focused_terminal(&mux, &attached);
                    resolve_inline_wheel_target(
                        &params,
                        active,
                        cursor_phys(&windows, 1.0),
                        8.0,
                        16.0,
                        1.0,
                    )
                },
            )
            .unwrap()
    }

    #[test]
    fn target_resolves_when_focused_inline_under_pointer() {
        let (mut app, _surface, child) = make_wheel_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_cursor(&mut app, Vec2::new(100.0, 100.0));

        let target = run_resolve_target(&mut app).expect("focused inline under pointer must hit");
        assert_eq!(target.child, child);
        // The host node spans the full window with no transform, so the
        // affine inverse maps (100, 100) to the same local point; the DIP is
        // the pointer minus the rect origin (phys 24, 32) at scale 1.
        assert!(
            target
                .position_dip
                .abs_diff_eq(Vec2::new(100.0 - 24.0, 100.0 - 32.0), 1e-3),
            "rect-local DIP mismatch: {:?}",
            target.position_dip,
        );
    }

    #[test]
    fn target_none_when_pointer_off_the_rect() {
        let (mut app, _surface, child) = make_wheel_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        // (400, 400) is inside the host node but outside the inline rect.
        set_cursor(&mut app, Vec2::new(400.0, 400.0));

        assert_eq!(
            run_resolve_target(&mut app),
            None,
            "a focused inline child not under the pointer must yield to terminal scrollback"
        );
    }

    #[test]
    fn target_none_when_inline_not_focused() {
        let (mut app, _surface, _child) = make_wheel_app();
        // FocusedWebview stays None: pointer over the rect, but no focus.
        set_cursor(&mut app, Vec2::new(100.0, 100.0));

        assert_eq!(
            run_resolve_target(&mut app),
            None,
            "wheel follows FOCUS — an unfocused inline rect under the pointer must scroll the terminal"
        );
    }

    #[test]
    fn target_none_when_focus_is_a_non_child_webview() {
        let (mut app, _surface, _child) = make_wheel_app();
        // Focus a webview that is NOT an inline child of the active surface
        // (e.g. a tab webview) — it carries no InlineWebview/ChildOf chain.
        let tab = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(tab);
        set_cursor(&mut app, Vec2::new(100.0, 100.0));

        assert_eq!(
            run_resolve_target(&mut app),
            None,
            "a focused non-inline (tab) webview must not claim the inline wheel"
        );
    }
}
