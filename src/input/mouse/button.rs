//! Mouse-button dispatch for every `OzmaTerminal` surface: app reporting, local
//! text selection + copy, and Cmd-click hyperlink open. Hit-tests the cursor to
//! a cell, drives the engine's pure `ButtonAction` router, and fans the decided
//! effects out via the shared `trigger_mouse_effects`. Registered by
//! `MouseButtonInputPlugin`; skips `MouseDisabled` surfaces.

use super::{
    CellContext, MouseEffect, TerminalSurfaces, cell_context_for, cell_pitch, hit_candidates,
    on_any_mouse_message, trigger_mouse_effects,
};
use crate::input::InputPhase;
use crate::input::bindings::OzmaMouseConfig;
use crate::input::current_modifiers;
use crate::input::gesture::{DragGesture, DragPhase, HeldPointer, OzmaMouseGesture};
use crate::input::hyperlink::link_modifier_held;
use crate::input::keyboard::current_terminal_modifiers;
use crate::surface::geometry::topmost_surface_at;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::time::{Real, Time};
use bevy::window::{CursorMoved, PrimaryWindow};
use ozma_tty_engine::{
    ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, CellCoord, Column, Line,
    MouseButtonKind, Point, ProtocolModifiers, Side, TermMode,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use std::time::Duration;

/// Registers the mouse-button dispatcher and its gesture resource. Runs in
/// `InputPhase::Dispatch`, gated to frames carrying any mouse message — the
/// focus/empty-candidate guard must still run on wheel-only frames to drain
/// readers and reset the gesture.
pub(super) struct MouseButtonInputPlugin;

impl Plugin for MouseButtonInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OzmaMouseGesture>().add_systems(
            Update,
            dispatch_mouse_buttons
                .in_set(InputPhase::Dispatch)
                .run_if(on_any_mouse_message()),
        );
    }
}

/// Per-frame constants computed once and threaded into the per-event and
/// per-drag helpers.
struct FrameContext {
    cursor_phys: Vec2,
    scale: f32,
    cell_w: f32,
    cell_h: f32,
    mods: ProtocolModifiers,
    modifier_held: bool,
}

/// The shared mouse-button dispatcher. Hit-tests the topmost terminal under the
/// cursor on press, locks drag/release to that terminal, tracks clicks and drag
/// state, drives `decide_button`, and fans the decided effects out to
/// per-operation `EntityEvent`s via `trigger_mouse_effects`. Skips any
/// `OzmaTerminal` carrying `MouseDisabled`; an empty candidate set (modal
/// suppression) drains events and resets the gesture.
fn dispatch_mouse_buttons(
    mut commands: Commands,
    mut gesture: ResMut<OzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut cursor_moved: MessageReader<CursorMoved>,
    terminals: TerminalSurfaces,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(frame) = resolve_frame(
        &mut gesture,
        &mut cursor_moved,
        &terminals,
        &windows,
        &metrics,
        &keys,
    ) else {
        buttons.clear();
        cursor_moved.clear();
        gesture.reset();
        return;
    };

    let now = time.elapsed();
    for ev in buttons.read() {
        process_button_event(
            &mut commands,
            &mut gesture,
            &terminals,
            &frame,
            &cfg,
            ev,
            now,
        );
    }
    synthesize_held_drag(&mut commands, &mut gesture, &terminals, &frame, &cfg);
}

/// Resolves the window guard and the per-frame cursor/constants for one run, or
/// `None` when the frame should be skipped (window missing/unfocused, empty
/// candidate set, or a cursor `effective_drag_cursor` rejects). On `None` the
/// caller drains the input readers and resets the gesture — this fn does not
/// (it reads `cursor_moved` only to refresh `last_cursor_phys`).
fn resolve_frame(
    gesture: &mut OzmaMouseGesture,
    cursor_moved: &mut MessageReader<CursorMoved>,
    terminals: &TerminalSurfaces<'_, '_>,
    windows: &Query<&Window, With<PrimaryWindow>>,
    metrics: &TerminalCellMetricsResource,
    keys: &ButtonInput<KeyCode>,
) -> Option<FrameContext> {
    let window = match windows.single() {
        Ok(window) if window.focused && !terminals.is_empty() => window,
        _ => {
            return None;
        }
    };
    let scale = window.scale_factor();
    let moved_phys = cursor_moved.read().last().map(|m| m.position * scale);
    let live = window.cursor_position().map(|c| c * scale);
    if let Some(latest) = live.or(moved_phys)
        && gesture.last_cursor_phys != Some(latest)
    {
        gesture.last_cursor_phys = Some(latest);
    }
    let active = gesture.held.is_some() || gesture.drag.is_some();
    let cursor_phys = effective_drag_cursor(live, active, gesture.last_cursor_phys)?;
    let (cell_w, cell_h) = cell_pitch(metrics);
    Some(FrameContext {
        cursor_phys,
        scale,
        cell_w,
        cell_h,
        mods: protocol_mods(keys),
        modifier_held: link_modifier_held(&current_modifiers(keys)),
    })
}

/// Resolves `target` to its `(CellContext, TermMode)` for this frame, or `None`
/// when the entity is no longer a live surface (the caller resets the gesture).
fn ctx_for<'a>(
    terminals: &'a TerminalSurfaces<'_, '_>,
    target: Entity,
    frame: &FrameContext,
) -> Option<(CellContext<'a>, TermMode)> {
    cell_context_for(terminals, target, frame.cell_w, frame.cell_h)
}

/// Processes one `MouseButtonInput`: hit-tests the target (press) or the locked
/// held entity (release), drives `resolve_button_event` + `decide_button`,
/// updates the held-pointer state, and triggers the decided effects.
fn process_button_event(
    commands: &mut Commands,
    gesture: &mut OzmaMouseGesture,
    terminals: &TerminalSurfaces<'_, '_>,
    frame: &FrameContext,
    cfg: &OzmaMouseConfig,
    ev: &MouseButtonInput,
    now: Duration,
) {
    let kind = button_kind(ev.state);
    let target = if kind == ButtonEventKind::Press {
        topmost_surface_at(frame.cursor_phys, hit_candidates(terminals))
    } else {
        gesture.held.map(|h| h.entity)
    };
    let Some(target) = target else {
        return;
    };
    let Some((ctx, modes)) = ctx_for(terminals, target, frame) else {
        gesture.reset();
        return;
    };
    let Some((evt, link)) = resolve_button_event(
        gesture,
        &ctx,
        ev,
        frame.cursor_phys,
        frame.scale,
        frame.modifier_held,
        now,
        cfg,
    ) else {
        return;
    };
    let decided = decide_button(
        gesture,
        modes,
        evt,
        frame.mods,
        frame.modifier_held,
        link,
        &cfg.buttons,
    );
    let opened = matches!(decided.as_slice(), [MouseEffect::OpenUri(_)]);
    match evt.kind {
        ButtonEventKind::Press if !opened => {
            gesture.held = Some(HeldPointer {
                entity: target,
                button: evt.button,
                last_cell: evt.cell,
            });
        }
        ButtonEventKind::Release => {
            gesture.held = None;
            gesture.last_cursor_phys = None;
        }
        _ => {}
    }
    trigger_mouse_effects(commands, target, decided);
}

/// Synthesizes a drag-motion effect for the held pointer when the cursor crossed
/// into a new cell, updating the held last-cell and triggering the effect. A
/// no-op when nothing is held; resets the gesture if the held surface is gone.
fn synthesize_held_drag(
    commands: &mut Commands,
    gesture: &mut OzmaMouseGesture,
    terminals: &TerminalSurfaces<'_, '_>,
    frame: &FrameContext,
    cfg: &OzmaMouseConfig,
) {
    let Some(held) = gesture.held else {
        return;
    };
    let Some((ctx, modes)) = ctx_for(terminals, held.entity, frame) else {
        gesture.reset();
        return;
    };
    if let Some((drag_effects, new_cell)) = synthesize_drag(
        gesture,
        &ctx,
        frame.cursor_phys,
        modes,
        frame.mods,
        frame.modifier_held,
        &cfg.buttons,
    ) {
        if let Some(h) = gesture.held.as_mut() {
            h.last_cell = new_cell;
        }
        trigger_mouse_effects(commands, held.entity, drag_effects);
    }
}

/// Pure per-event decision for a mouse button. Mutates `gesture` (drag phase /
/// click state) and returns the effects to apply. A Cmd/Ctrl-click on a linked
/// cell opens the URL and consumes the event; otherwise the engine's
/// `ButtonAction::route` decides forward-to-app vs local selection.
fn decide_button(
    gesture: &mut OzmaMouseGesture,
    modes: TermMode,
    evt: ButtonEvent,
    mods: ProtocolModifiers,
    modifier_held: bool,
    link_at_cell: Option<String>,
    cfg: &ButtonConfig,
) -> Vec<MouseEffect> {
    if evt.kind == ButtonEventKind::Press
        && evt.button == MouseButtonKind::Left
        && modifier_held
        && let Some(uri) = link_at_cell
    {
        return vec![MouseEffect::OpenUri(uri)];
    }

    let mut effects = match ButtonAction::route(modes, evt, mods, cfg) {
        ButtonAction::Noop => Vec::new(),
        ButtonAction::WriteToPty(b) => vec![MouseEffect::Write(b)],
        ButtonAction::ClearAndWriteToPty(b) => vec![MouseEffect::SelClear, MouseEffect::Write(b)],
        ButtonAction::ArmDrag { ty, cell, side } => {
            gesture.drag = Some(DragGesture {
                origin: cell,
                side,
                ty,
                phase: DragPhase::Armed,
            });
            vec![MouseEffect::SelClear]
        }
        ButtonAction::StartLocalSelection { ty, cell, side } => {
            gesture.drag = Some(DragGesture {
                origin: cell,
                side,
                ty,
                phase: DragPhase::Started,
            });
            vec![MouseEffect::SelStart {
                point: to_viewport_point(cell),
                side,
                ty,
            }]
        }
        ButtonAction::UpdateLocalSelection { cell, side } => update_selection(gesture, cell, side),
        ButtonAction::ClearLocalSelection => {
            gesture.drag = None;
            vec![MouseEffect::SelClear]
        }
    };

    if evt.kind == ButtonEventKind::Release && evt.button == MouseButtonKind::Left {
        if effects.is_empty() && matches!(&gesture.drag, Some(d) if d.phase == DragPhase::Started) {
            effects.push(MouseEffect::Copy);
        }
        gesture.drag = None;
    }
    effects
}

/// The physical cursor position to drive the gesture with this frame.
///
/// `live` is `window.cursor_position()` (already `None` once the pointer leaves
/// the window, since Bevy bounds-masks off-window positions); `active` is
/// whether a gesture is in flight (a button is held or a drag is started);
/// `last` is the last observed physical position. Returns the live position
/// when present, the last-known position while a gesture is active (so an
/// off-window drag keeps extending), or `None` when idle with no cursor (the
/// caller then resets the gesture).
fn effective_drag_cursor(live: Option<Vec2>, active: bool, last: Option<Vec2>) -> Option<Vec2> {
    match (live, active) {
        (Some(c), _) => Some(c),
        (None, true) => last,
        (None, false) => None,
    }
}

/// Builds `ProtocolModifiers` from the held keys.
fn protocol_mods(keys: &ButtonInput<KeyCode>) -> ProtocolModifiers {
    let m = current_terminal_modifiers(keys);
    ProtocolModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Converts a 1-indexed protocol `CellCoord` into the engine's viewport-relative
/// selection `Point` (row 0 = top of viewport; the engine translates for scroll).
fn to_viewport_point(cell: CellCoord) -> Point {
    Point::new(Line(cell.row as i32 - 1), Column(cell.col as usize - 1))
}

/// Resolves one `MouseButtonInput` to a `ButtonEvent` + optional link URI, or
/// `None` when it maps to no terminal button or no cell. Encapsulates button
/// mapping, the off-node release fallback, click-count registration, and the
/// modifier-gated hyperlink lookup.
fn resolve_button_event(
    gesture: &mut OzmaMouseGesture,
    ctx: &CellContext,
    ev: &MouseButtonInput,
    cursor_phys: Vec2,
    scale: f32,
    modifier_held: bool,
    now: Duration,
    cfg: &OzmaMouseConfig,
) -> Option<(ButtonEvent, Option<String>)> {
    let button = map_button(ev.button)?;
    let kind = button_kind(ev.state);
    // NOTE: a release with the cursor off the terminal node must still be
    // processed (via the last tracked cell) — otherwise `held`/`drag` stick and
    // later cursor motion replays stale selection / forward reports.
    let release_fallback = (kind == ButtonEventKind::Release)
        .then(|| gesture.held.map(|h| (h.last_cell, Side::Left)))
        .flatten();
    let (cell, side) = ctx.hit(cursor_phys).or(release_fallback)?;
    let click_count = if kind == ButtonEventKind::Press {
        gesture.click.register(
            now,
            cursor_phys / scale,
            (cfg.double_click_timeout, cfg.click_drift_px),
        )
    } else {
        1
    };
    let link = (kind == ButtonEventKind::Press && button == MouseButtonKind::Left && modifier_held)
        .then(|| {
            ctx.grid
                .hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16)
                .map(|(_id, uri)| uri.as_str().to_string())
        })
        .flatten();
    Some((
        ButtonEvent {
            kind,
            button,
            cell,
            side,
            click_count,
        },
        link,
    ))
}

/// Synthesizes a drag-motion effect set when a held pointer crosses into a new
/// cell. Returns the decided effects and the new last-cell to record, or `None`
/// when no button is held, the pointer is off-node, or it has not moved.
fn synthesize_drag(
    gesture: &mut OzmaMouseGesture,
    ctx: &CellContext,
    cursor_phys: Vec2,
    modes: TermMode,
    mods: ProtocolModifiers,
    modifier_held: bool,
    cfg: &ButtonConfig,
) -> Option<(Vec<MouseEffect>, CellCoord)> {
    let held = gesture.held?;
    let (cell, side) = ctx.hit(cursor_phys)?;
    if held.last_cell == cell {
        return None;
    }
    let evt = ButtonEvent {
        kind: ButtonEventKind::Drag,
        button: held.button,
        cell,
        side,
        click_count: 1,
    };
    let effects = decide_button(gesture, modes, evt, mods, modifier_held, None, cfg);
    Some((effects, cell))
}

/// Lazily materializes an armed selection on the first cell change, then extends.
fn update_selection(
    gesture: &mut OzmaMouseGesture,
    cell: CellCoord,
    side: Side,
) -> Vec<MouseEffect> {
    let Some(drag) = gesture.drag.as_mut() else {
        return Vec::new();
    };
    match drag.phase {
        DragPhase::Armed => {
            if cell == drag.origin {
                return Vec::new();
            }
            let origin = drag.origin;
            let ty = drag.ty;
            let origin_side = drag.side;
            drag.phase = DragPhase::Started;
            vec![
                MouseEffect::SelStart {
                    point: to_viewport_point(origin),
                    side: origin_side,
                    ty,
                },
                MouseEffect::SelUpdate {
                    point: to_viewport_point(cell),
                    side,
                },
            ]
        }
        DragPhase::Started => {
            vec![MouseEffect::SelUpdate {
                point: to_viewport_point(cell),
                side,
            }]
        }
    }
}

fn map_button(b: MouseButton) -> Option<MouseButtonKind> {
    match b {
        MouseButton::Left => Some(MouseButtonKind::Left),
        MouseButton::Middle => Some(MouseButtonKind::Middle),
        MouseButton::Right => Some(MouseButtonKind::Right),
        _ => None,
    }
}

fn button_kind(state: ButtonState) -> ButtonEventKind {
    match state {
        ButtonState::Pressed => ButtonEventKind::Press,
        ButtonState::Released => ButtonEventKind::Release,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::Clipboard;
    use crate::input::focus::MouseDisabled;
    use crate::input::mouse::test_support::{
        CapturedEffects, add_effect_capture_observers, set_phys_cursor, test_metrics,
    };
    use crate::surface::OzmaTerminal;
    use bevy::ui::{ComputedNode, UiGlobalTransform};
    use ozma_tty_engine::SelectionType;
    use ozma_tty_engine::TerminalHandle;
    use ozma_tty_renderer::schema::TerminalGrid;

    fn make_selection_app() -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<OzmaMouseConfig>()
            .init_resource::<OzmaMouseGesture>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Clipboard>()
            .init_resource::<CapturedEffects>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_buttons);
        add_effect_capture_observers(&mut app);

        let handle = TerminalHandle::detached(100, 37);
        // Node at window center (400,300), size 800x600 -> top-left (0,0).
        app.world_mut().spawn((
            OzmaTerminal,
            handle,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 300.0),
            TerminalGrid {
                cols: 100,
                rows: 37,
                ..default()
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn write_cursor_moved(app: &mut App, pos: Vec2) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CursorMoved>>()
            .write(CursorMoved {
                window: Entity::PLACEHOLDER,
                position: pos,
                delta: None,
            });
    }

    fn write_left(app: &mut App, state: ButtonState) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    fn ev(kind: ButtonEventKind, col: u32, row: u32, count: u8) -> ButtonEvent {
        ButtonEvent {
            kind,
            button: MouseButtonKind::Left,
            cell: CellCoord { col, row },
            side: Side::Left,
            click_count: count,
        }
    }

    #[test]
    fn effective_drag_cursor_truth_table() {
        let live = Vec2::new(10.0, 10.0);
        let last = Vec2::new(99.0, 88.0);
        // Live cursor present: always use it, regardless of gesture state.
        assert_eq!(
            effective_drag_cursor(Some(live), false, Some(last)),
            Some(live)
        );
        assert_eq!(
            effective_drag_cursor(Some(live), true, Some(last)),
            Some(live)
        );
        // Off-window (live None) while a gesture is active: fall back to last-known.
        assert_eq!(effective_drag_cursor(None, true, Some(last)), Some(last));
        assert_eq!(effective_drag_cursor(None, true, None), None);
        // Off-window while idle: nothing to drive — caller resets.
        assert_eq!(effective_drag_cursor(None, false, Some(last)), None);
    }

    #[test]
    fn drag_survives_cursor_leaving_window() {
        let mut app = make_selection_app();

        // Press inside (cell ~col 6, row 4).
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();

        // Drag to a new cell inside -> selection started.
        set_phys_cursor(&mut app, Vec2::new(80.0, 48.0));
        app.update();
        assert!(
            matches!(
                app.world().resource::<OzmaMouseGesture>().drag,
                Some(DragGesture {
                    phase: DragPhase::Started,
                    ..
                })
            ),
            "dragging across a cell must start the selection"
        );

        // Leave the window: physical (900,700) is out of the 800x600 bounds, so
        // cursor_position() returns None, but CursorMoved still carries the position.
        app.world_mut().resource_mut::<CapturedEffects>().0.clear();
        set_phys_cursor(&mut app, Vec2::new(900.0, 700.0));
        write_cursor_moved(&mut app, Vec2::new(900.0, 700.0));
        app.update();

        let g = app.world().resource::<OzmaMouseGesture>();
        assert!(
            g.held.is_some(),
            "leaving the window must NOT drop the held pointer"
        );
        assert!(
            matches!(
                g.drag,
                Some(DragGesture {
                    phase: DragPhase::Started,
                    ..
                })
            ),
            "leaving the window must NOT cancel the in-progress selection"
        );

        let cap = app.world().resource::<CapturedEffects>();
        let pinned = cap
            .0
            .iter()
            .any(|e| matches!(e, MouseEffect::SelUpdate { point, .. } if point.column.0 == 99));
        assert!(
            pinned,
            "the selection must extend (pin) to the rightmost edge column while \
             outside, got {:?}",
            cap.0
        );
    }

    #[test]
    fn release_after_leaving_window_copies() {
        let mut app = make_selection_app();

        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();
        set_phys_cursor(&mut app, Vec2::new(80.0, 48.0));
        app.update();

        // Leave the window.
        set_phys_cursor(&mut app, Vec2::new(900.0, 700.0));
        write_cursor_moved(&mut app, Vec2::new(900.0, 700.0));
        app.update();

        // Release while still outside.
        app.world_mut().resource_mut::<CapturedEffects>().0.clear();
        write_left(&mut app, ButtonState::Released);
        app.update();

        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().any(|e| matches!(e, MouseEffect::Copy)),
            "releasing after leaving the window must copy the selection, got {:?}",
            cap.0
        );
        let g = app.world().resource::<OzmaMouseGesture>();
        assert!(
            g.held.is_none() && g.drag.is_none(),
            "release must end the gesture"
        );
    }

    #[test]
    fn idle_cursor_outside_window_resets_and_clears_last() {
        let mut app = make_selection_app();

        // No press; cursor is out of bounds.
        set_phys_cursor(&mut app, Vec2::new(900.0, 700.0));
        write_cursor_moved(&mut app, Vec2::new(900.0, 700.0));
        app.update();

        let g = app.world().resource::<OzmaMouseGesture>();
        assert!(
            g.drag.is_none() && g.held.is_none(),
            "an idle frame with no in-window cursor must stay reset"
        );
        assert!(
            g.last_cursor_phys.is_none(),
            "the idle reset must clear last_cursor_phys"
        );
    }

    #[test]
    fn mouse_disabled_terminal_drains_without_arming_a_gesture() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<OzmaMouseConfig>()
            .init_resource::<OzmaMouseGesture>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Clipboard>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_buttons);
        app.world_mut().spawn((OzmaTerminal, MouseDisabled));
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
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert!(app.world().resource::<OzmaMouseGesture>().drag.is_none());
    }

    #[test]
    fn local_single_press_arms_drag_and_clears() {
        let mut g = OzmaMouseGesture::default();
        let fx = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
        );
        assert_eq!(fx, vec![MouseEffect::SelClear]);
        assert!(matches!(
            g.drag,
            Some(DragGesture {
                phase: DragPhase::Armed,
                ..
            })
        ));
    }

    #[test]
    fn local_drag_materializes_then_extends() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig {
            max_protocol_events_per_frame: 8,
        };
        decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        let fx = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Drag, 7, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        assert_eq!(
            fx,
            vec![
                MouseEffect::SelStart {
                    point: to_viewport_point(CellCoord { col: 5, row: 5 }),
                    side: Side::Left,
                    ty: SelectionType::Simple
                },
                MouseEffect::SelUpdate {
                    point: to_viewport_point(CellCoord { col: 7, row: 5 }),
                    side: Side::Left
                },
            ]
        );
        let fx2 = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Drag, 9, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        assert_eq!(
            fx2,
            vec![MouseEffect::SelUpdate {
                point: to_viewport_point(CellCoord { col: 9, row: 5 }),
                side: Side::Left
            }]
        );
    }

    #[test]
    fn release_after_drag_copies() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig {
            max_protocol_events_per_frame: 8,
        };
        decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Drag, 7, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        let fx = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Release, 7, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        assert_eq!(fx, vec![MouseEffect::Copy]);
        assert!(g.drag.is_none());
    }

    #[test]
    fn release_after_bare_click_does_not_copy() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig {
            max_protocol_events_per_frame: 8,
        };
        decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        let fx = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Release, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        assert_eq!(fx, vec![]);
        assert!(g.drag.is_none());
    }

    #[test]
    fn double_click_starts_word_selection() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig {
            max_protocol_events_per_frame: 8,
        };
        let fx = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Press, 5, 5, 2),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        assert_eq!(
            fx,
            vec![MouseEffect::SelStart {
                point: to_viewport_point(CellCoord { col: 5, row: 5 }),
                side: Side::Left,
                ty: SelectionType::Semantic
            }]
        );
    }

    #[test]
    fn app_capture_press_forwards_sgr_bytes() {
        let mut g = OzmaMouseGesture::default();
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let fx = decide_button(
            &mut g,
            modes,
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
        );
        assert_eq!(
            fx,
            vec![
                MouseEffect::SelClear,
                MouseEffect::Write(b"\x1b[<0;5;5M".to_vec())
            ]
        );
    }

    #[test]
    fn app_capture_drag_forwards_motion_bytes() {
        let mut g = OzmaMouseGesture::default();
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let cfg = ButtonConfig {
            max_protocol_events_per_frame: 8,
        };
        decide_button(
            &mut g,
            modes,
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        let fx = decide_button(
            &mut g,
            modes,
            ev(ButtonEventKind::Drag, 6, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
            &cfg,
        );
        assert!(
            matches!(fx.as_slice(), [MouseEffect::Write(b)] if b.starts_with(b"\x1b[<32;")),
            "a drag under app capture must forward a motion report (SGR cb motion bit 32), got {fx:?}"
        );
    }

    #[test]
    fn shift_bypass_selects_even_when_captured() {
        let mut g = OzmaMouseGesture::default();
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let mods = ProtocolModifiers {
            shift: true,
            ..Default::default()
        };
        let fx = decide_button(
            &mut g,
            modes,
            ev(ButtonEventKind::Press, 5, 5, 1),
            mods,
            false,
            None,
            &ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
        );
        assert_eq!(fx, vec![MouseEffect::SelClear]);
        assert!(matches!(
            g.drag,
            Some(DragGesture {
                phase: DragPhase::Armed,
                ..
            })
        ));
    }

    #[test]
    fn cmd_click_on_link_opens_and_consumes() {
        let mut g = OzmaMouseGesture::default();
        let fx = decide_button(
            &mut g,
            TermMode::empty(),
            ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers {
                meta: true,
                ..Default::default()
            },
            true,
            Some("https://example.com".into()),
            &ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
        );
        assert_eq!(fx, vec![MouseEffect::OpenUri("https://example.com".into())]);
        assert!(g.drag.is_none(), "a link-open press must not arm a drag");
    }

    #[test]
    fn to_viewport_point_zero_indexes_the_one_indexed_cell() {
        let p = to_viewport_point(CellCoord { col: 5, row: 3 });
        assert_eq!(p.line.0, 2);
        assert_eq!(p.column.0, 4);
    }

    #[test]
    fn protocol_mods_sets_ctrl_and_shift() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::ShiftLeft);
        let mods = protocol_mods(&keys);
        assert!(mods.ctrl);
        assert!(mods.shift);
        assert!(!mods.alt);
        assert!(!mods.meta);
    }
}
