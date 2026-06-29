//! Mouse-button dispatch for every `OzmaTerminal` surface: app reporting, local
//! text selection + copy, and Cmd-click hyperlink open. Hit-tests the cursor to
//! a cell, drives the engine's pure `ButtonAction` router, and fans the decided
//! effects out via the shared `trigger_mouse_effects`. Registered by
//! `MouseButtonInputPlugin`; skips `MouseDisabled` surfaces.

use super::{CellContext, MouseEffect, trigger_mouse_effects};
use crate::input::InputPhase;
use crate::input::bindings::OzmaMouseConfig;
use crate::input::current_modifiers;
use crate::input::focus::MouseDisabled;
use crate::input::gesture::{DragGesture, DragPhase, HeldPointer, OzmaMouseGesture};
use crate::input::hyperlink::link_modifier_held;
use crate::input::keyboard::current_terminal_modifiers;
use crate::webview_pointer::topmost_surface_at;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::time::{Real, Time};
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{
    ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, CellCoord, Column, Line,
    MouseButtonKind, Point, ProtocolModifiers, Side, TermMode, TerminalHandle,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::TerminalGrid;
use std::time::Duration;

/// Registers the mouse-button dispatcher and its gesture resource. Runs in
/// `InputPhase::Dispatch`, gated to frames carrying a button or cursor-move
/// message.
pub(super) struct MouseButtonInputPlugin;

impl Plugin for MouseButtonInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OzmaMouseGesture>().add_systems(
            Update,
            dispatch_mouse_buttons
                .in_set(InputPhase::Dispatch)
                .run_if(on_message::<MouseButtonInput>.or(on_message::<CursorMoved>)),
        );
    }
}

/// The shared mouse-button dispatcher. Hit-tests the topmost terminal under the
/// cursor on press, locks drag/release to that terminal, tracks clicks and drag
/// state, drives `decide_button`, and fans the decided effects out to
/// per-operation `EntityEvent`s via `trigger_mouse_effects`. Skips any
/// `OzmaTerminal` carrying `MouseDisabled`; an empty candidate set (modal
/// suppression) drains events and resets the gesture.
pub(super) fn dispatch_mouse_buttons(
    mut commands: Commands,
    mut gesture: ResMut<OzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut cursor_moved: MessageReader<CursorMoved>,
    terminals: Query<
        (
            Entity,
            &TerminalHandle,
            &ComputedNode,
            &UiGlobalTransform,
            &TerminalGrid,
        ),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let window = match windows.single() {
        Ok(window) if window.focused => window,
        _ => {
            buttons.clear();
            cursor_moved.clear();
            gesture.reset();
            return;
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
    let Some(cursor_phys) = effective_drag_cursor(live, active, gesture.last_cursor_phys) else {
        buttons.clear();
        gesture.reset();
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let mods = protocol_mods(&keys);
    let modifier_held = link_modifier_held(&current_modifiers(&keys));

    for ev in buttons.read() {
        let kind = match ev.state {
            ButtonState::Pressed => ButtonEventKind::Press,
            ButtonState::Released => ButtonEventKind::Release,
        };
        let target = if kind == ButtonEventKind::Press {
            topmost_surface_at(
                cursor_phys,
                terminals
                    .iter()
                    .map(|(e, _, node, transform, _)| (e, node, transform)),
            )
        } else {
            gesture.held.map(|h| h.entity)
        };
        let Some(target) = target else {
            continue;
        };
        let Ok((_, handle, node, transform, grid)) = terminals.get(target) else {
            gesture.reset();
            continue;
        };
        let ctx = CellContext {
            node,
            transform,
            grid,
            cell_w,
            cell_h,
        };
        let modes = handle.current_modes();
        let Some((evt, link)) = resolve_button_event(
            &mut gesture,
            &ctx,
            ev,
            cursor_phys,
            scale,
            modifier_held,
            time.elapsed(),
            &cfg,
        ) else {
            continue;
        };
        let decided = decide_button(
            &mut gesture,
            modes,
            evt,
            mods,
            modifier_held,
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
        trigger_mouse_effects(&mut commands, target, decided);
    }

    let Some(held) = gesture.held else {
        return;
    };
    let Ok((_, handle, node, transform, grid)) = terminals.get(held.entity) else {
        gesture.reset();
        return;
    };
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };
    let modes = handle.current_modes();
    if let Some((drag_effects, new_cell)) = synthesize_drag(
        &mut gesture,
        &ctx,
        cursor_phys,
        modes,
        mods,
        modifier_held,
        &cfg.buttons,
    ) {
        if let Some(h) = gesture.held.as_mut() {
            h.last_cell = new_cell;
        }
        trigger_mouse_effects(&mut commands, held.entity, drag_effects);
    }
}

/// Pure per-event decision for a mouse button. Mutates `gesture` (drag phase /
/// click state) and returns the effects to apply. A Cmd/Ctrl-click on a linked
/// cell opens the URL and consumes the event; otherwise the engine's
/// `ButtonAction::route` decides forward-to-app vs local selection.
pub(super) fn decide_button(
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
pub(super) fn effective_drag_cursor(
    live: Option<Vec2>,
    active: bool,
    last: Option<Vec2>,
) -> Option<Vec2> {
    match (live, active) {
        (Some(c), _) => Some(c),
        (None, true) => last,
        (None, false) => None,
    }
}

/// Builds `ProtocolModifiers` from the held keys.
pub(super) fn protocol_mods(keys: &ButtonInput<KeyCode>) -> ProtocolModifiers {
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
pub(super) fn to_viewport_point(cell: CellCoord) -> Point {
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
    let kind = match ev.state {
        ButtonState::Pressed => ButtonEventKind::Press,
        ButtonState::Released => ButtonEventKind::Release,
    };
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
