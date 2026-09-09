//! Mouse-button dispatch for every `OrzmaTerminal` surface: local text
//! selection + copy, Cmd-click hyperlink open, and click-to-focus. Hit-tests
//! the cursor to a cell, drives the local-only `LocalButtonAction::route`
//! router, and fans effects out via the shared `trigger_mouse_effects`. A
//! press that a URI open did not consume also triggers `PaneClicked` on the
//! target surface. App-forward mouse reporting is out of scope until mouse
//! routing is reintroduced against `orzma_tty` (D17 of the engine-swap
//! design). Registered by `MouseButtonInputPlugin`; skips `MouseDisabled`
//! surfaces.

use super::{
    CellContext, MouseEffect, TerminalSurfaces, cell_context_for, cell_dims, hit_candidates,
    on_any_mouse_message, trigger_mouse_effects,
};
use crate::input::InputPhase;
use crate::input::bindings::OrzmaMouseConfig;
use crate::input::current_modifiers;
use crate::input::focus::PaneClicked;
use crate::input::hyperlink::link_modifier_held;
use crate::input::keyboard::current_terminal_modifiers;
use crate::input::mouse::gesture::{DragGesture, DragPhase, HeldPointer, OrzmaMouseGesture};
use crate::surface::geometry::topmost_surface_at;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::time::{Real, Time};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_orzmux::prelude::{CellSide, GridPoint, SelectionKind};
use orzma_tty::prelude::{CellCoord, MouseReportKind, ProtocolModifiers};
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_vt::prelude::{GridColumn, GridLine};
use std::time::Duration;

/// Registers the mouse-button dispatcher and its gesture resource. Runs in
/// `InputPhase::Dispatch`, gated to frames carrying any mouse message — the
/// focus/empty-candidate guard must still run on wheel-only frames to drain
/// readers and reset the gesture.
pub(super) struct MouseButtonInputPlugin;

impl Plugin for MouseButtonInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrzmaMouseGesture>().add_systems(
            Update,
            dispatch_mouse_buttons
                .in_set(InputPhase::Dispatch)
                .run_if(on_any_mouse_message()),
        );
    }
}

/// Logical mouse-button identity accepted by the local-selection router.
///
/// Deliberately narrower than [`orzma_tty::prelude::MouseButton`]: only the
/// three physical buttons a Bevy `MouseButtonInput` can carry, with no wheel
/// variants to exhaustively (and uselessly) match against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::input::mouse) enum MouseButtonKind {
    Left,
    Middle,
    Right,
}

/// One mouse-button event, projected into pane-relative cell coords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ButtonEvent {
    kind: MouseReportKind,
    button: MouseButtonKind,
    cell: CellCoord,
    side: CellSide,
    /// 1, 2, or 3 — caller-tracked. Drag and Release ignore this.
    click_count: u8,
}

/// What [`LocalButtonAction::route`] decided for the local-selection path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalButtonAction {
    /// Nothing to do for this event: a release on the local path, or a
    /// middle / right button, which has no primary-selection paste.
    Noop,
    /// A single-click left-press has occurred. The caller should arm a
    /// pending drag at `(cell, side)` of granularity `kind` and clear any
    /// pre-existing local selection, but NOT start the selection yet — it
    /// materializes lazily on the first `UpdateLocalSelection` whose cell
    /// differs from the armed `cell`.
    ArmDrag {
        kind: SelectionKind,
        cell: CellCoord,
        side: CellSide,
    },
    /// Begin a new local selection of `kind` at `(cell, side)`.
    StartLocalSelection {
        kind: SelectionKind,
        cell: CellCoord,
        side: CellSide,
    },
    /// Extend the current local selection's moving end to `(cell, side)`.
    UpdateLocalSelection { cell: CellCoord, side: CellSide },
}

impl LocalButtonAction {
    /// Ports the local-selection branches of the removed engine's
    /// `ButtonAction::route`. The app-forward branch (mouse-mode PTY
    /// reporting) is not ported — that is out of scope until mouse routing
    /// returns against `orzma_tty` (D17b).
    fn route(evt: ButtonEvent, mods: ProtocolModifiers) -> Self {
        match (evt.kind, evt.button) {
            (MouseReportKind::Press, MouseButtonKind::Left) => {
                if mods.alt {
                    // TODO: switch to `SelectionKind::Block` once `orzma_vt`
                    // gains it (D16); an Alt+click rounds down to `Lines`
                    // until then.
                    return Self::StartLocalSelection {
                        kind: SelectionKind::Lines,
                        cell: evt.cell,
                        side: evt.side,
                    };
                }
                match evt.click_count {
                    1 => Self::ArmDrag {
                        kind: SelectionKind::Simple,
                        cell: evt.cell,
                        side: evt.side,
                    },
                    // TODO: switch to a word-snapped Semantic kind once
                    // `orzma_vt` gains one (D16); a double-click rounds down
                    // to plain `Simple` until then.
                    2 => Self::StartLocalSelection {
                        kind: SelectionKind::Simple,
                        cell: evt.cell,
                        side: evt.side,
                    },
                    _ => Self::StartLocalSelection {
                        kind: SelectionKind::Lines,
                        cell: evt.cell,
                        side: evt.side,
                    },
                }
            }
            (MouseReportKind::Drag, MouseButtonKind::Left) => Self::UpdateLocalSelection {
                cell: evt.cell,
                side: evt.side,
            },
            (MouseReportKind::Release, MouseButtonKind::Left) => Self::Noop,
            _ => Self::Noop,
        }
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
/// `OrzmaTerminal` carrying `MouseDisabled`; an empty candidate set (modal
/// suppression) drains events and resets the gesture.
fn dispatch_mouse_buttons(
    mut commands: Commands,
    mut gesture: ResMut<OrzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut cursor_moved: MessageReader<CursorMoved>,
    terminals: TerminalSurfaces,
    cfg: Res<OrzmaMouseConfig>,
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
    synthesize_held_drag(&mut commands, &mut gesture, &terminals, &frame);
}

/// Resolves the window guard and the per-frame cursor/constants for one run, or
/// `None` when the frame should be skipped (window missing/unfocused, empty
/// candidate set, or a cursor `effective_drag_cursor` rejects). On `None` the
/// caller drains the input readers and resets the gesture — this fn does not
/// (it reads `cursor_moved` only to refresh `last_cursor_phys`).
fn resolve_frame(
    gesture: &mut OrzmaMouseGesture,
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
    let (cell_w, cell_h) = cell_dims(metrics);
    Some(FrameContext {
        cursor_phys,
        scale,
        cell_w,
        cell_h,
        mods: protocol_mods(keys),
        modifier_held: link_modifier_held(&current_modifiers(keys)),
    })
}

/// Processes one `MouseButtonInput`: hit-tests the target (press) or the locked
/// held entity (release), drives `resolve_button_event` + `decide_button`,
/// updates the held-pointer state, and triggers the decided effects.
fn process_button_event(
    commands: &mut Commands,
    gesture: &mut OrzmaMouseGesture,
    terminals: &TerminalSurfaces<'_, '_>,
    frame: &FrameContext,
    cfg: &OrzmaMouseConfig,
    ev: &MouseButtonInput,
    now: Duration,
) {
    let kind = button_kind(ev.state);
    let target = if kind == MouseReportKind::Press {
        topmost_surface_at(frame.cursor_phys, hit_candidates(terminals))
    } else {
        gesture.held.map(|h| h.entity)
    };
    let Some(target) = target else {
        return;
    };
    let Some(ctx) = cell_context_for(terminals, target, frame.cell_w, frame.cell_h) else {
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
    let decided = decide_button(gesture, evt, frame.mods, frame.modifier_held, link);
    let opened = matches!(decided.as_slice(), [MouseEffect::OpenUri(_)]);
    match evt.kind {
        MouseReportKind::Press if !opened => {
            commands.trigger(PaneClicked { entity: target });
            gesture.held = Some(HeldPointer {
                entity: target,
                button: evt.button,
                last_cell: evt.cell,
            });
        }
        MouseReportKind::Release => {
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
    gesture: &mut OrzmaMouseGesture,
    terminals: &TerminalSurfaces<'_, '_>,
    frame: &FrameContext,
) {
    let Some(held) = gesture.held else {
        return;
    };
    let Some(ctx) = cell_context_for(terminals, held.entity, frame.cell_w, frame.cell_h) else {
        gesture.reset();
        return;
    };
    if let Some((drag_effects, new_cell)) = synthesize_drag(
        gesture,
        &ctx,
        frame.cursor_phys,
        frame.mods,
        frame.modifier_held,
    ) {
        if let Some(h) = gesture.held.as_mut() {
            h.last_cell = new_cell;
        }
        trigger_mouse_effects(commands, held.entity, drag_effects);
    }
}

/// Pure per-event decision for a mouse button. Mutates `gesture` (drag phase /
/// click state) and returns the effects to apply. A Cmd/Ctrl-click on a linked
/// cell opens the URL and consumes the event; otherwise
/// `LocalButtonAction::route` decides the local-selection response —
/// app-forward reporting is out of scope (D17b of the engine-swap design).
fn decide_button(
    gesture: &mut OrzmaMouseGesture,
    evt: ButtonEvent,
    mods: ProtocolModifiers,
    modifier_held: bool,
    link_at_cell: Option<String>,
) -> Vec<MouseEffect> {
    if evt.kind == MouseReportKind::Press
        && evt.button == MouseButtonKind::Left
        && modifier_held
        && let Some(uri) = link_at_cell
    {
        return vec![MouseEffect::OpenUri(uri)];
    }

    let mut effects = match LocalButtonAction::route(evt, mods) {
        LocalButtonAction::Noop => Vec::new(),
        LocalButtonAction::ArmDrag { kind, cell, side } => {
            gesture.drag = Some(DragGesture {
                origin: cell,
                side,
                ty: kind,
                phase: DragPhase::Armed,
            });
            vec![MouseEffect::SelClear]
        }
        LocalButtonAction::StartLocalSelection { kind, cell, side } => {
            gesture.drag = Some(DragGesture {
                origin: cell,
                side,
                ty: kind,
                phase: DragPhase::Started,
            });
            vec![MouseEffect::SelStart {
                point: to_grid_point(cell),
                side,
                ty: kind,
            }]
        }
        LocalButtonAction::UpdateLocalSelection { cell, side } => {
            update_selection(gesture, cell, side)
        }
    };

    if evt.kind == MouseReportKind::Release && evt.button == MouseButtonKind::Left {
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

/// Converts a 1-indexed protocol `CellCoord` into a 0-indexed,
/// viewport-relative `GridPoint` (row 0 = top of the currently displayed
/// viewport). This dispatcher has no read access to the VT (a pane entity's
/// `OrzmuxPane` names the backend pane, but the VT itself lives on the
/// multiplexer backend thread, a backend that may later run out of
/// process), so it cannot resolve scrollback itself —
/// `action/terminal/selection.rs`'s apply observer offsets this by the
/// terminal's live display offset before firing `RequestTtySelectionStart`
/// / `RequestTtySelectionUpdate`.
fn to_grid_point(cell: CellCoord) -> GridPoint {
    GridPoint {
        line: GridLine(cell.row as i32 - 1),
        column: GridColumn((cell.col - 1) as u16),
    }
}

/// Resolves one `MouseButtonInput` to a `ButtonEvent` + optional link URI, or
/// `None` when it maps to no terminal button or no cell. Encapsulates button
/// mapping, the off-node release fallback, click-count registration, and the
/// modifier-gated hyperlink lookup.
fn resolve_button_event(
    gesture: &mut OrzmaMouseGesture,
    ctx: &CellContext,
    ev: &MouseButtonInput,
    cursor_phys: Vec2,
    scale: f32,
    modifier_held: bool,
    now: Duration,
    cfg: &OrzmaMouseConfig,
) -> Option<(ButtonEvent, Option<String>)> {
    let button = map_button(ev.button)?;
    let kind = button_kind(ev.state);
    // NOTE: a release with the cursor off the terminal node must still be
    // processed (via the last tracked cell) — otherwise `held`/`drag` stick and
    // later cursor motion replays stale selection / forward reports.
    let release_fallback = (kind == MouseReportKind::Release)
        .then(|| gesture.held.map(|h| (h.last_cell, CellSide::Left)))
        .flatten();
    let (cell, side) = ctx.hit(cursor_phys).or(release_fallback)?;
    let click_count = if kind == MouseReportKind::Press {
        gesture.click.register(
            now,
            cursor_phys / scale,
            (cfg.double_click_timeout, cfg.click_drift_px),
        )
    } else {
        1
    };
    let link = (kind == MouseReportKind::Press && button == MouseButtonKind::Left && modifier_held)
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
    gesture: &mut OrzmaMouseGesture,
    ctx: &CellContext,
    cursor_phys: Vec2,
    mods: ProtocolModifiers,
    modifier_held: bool,
) -> Option<(Vec<MouseEffect>, CellCoord)> {
    let held = gesture.held?;
    let (cell, side) = ctx.hit(cursor_phys)?;
    if held.last_cell == cell {
        return None;
    }
    let evt = ButtonEvent {
        kind: MouseReportKind::Drag,
        button: held.button,
        cell,
        side,
        click_count: 1,
    };
    let effects = decide_button(gesture, evt, mods, modifier_held, None);
    Some((effects, cell))
}

/// Lazily materializes an armed selection on the first cell change, then extends.
fn update_selection(
    gesture: &mut OrzmaMouseGesture,
    cell: CellCoord,
    side: CellSide,
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
                    point: to_grid_point(origin),
                    side: origin_side,
                    ty,
                },
                MouseEffect::SelUpdate {
                    point: to_grid_point(cell),
                    side,
                },
            ]
        }
        DragPhase::Started => {
            vec![MouseEffect::SelUpdate {
                point: to_grid_point(cell),
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

fn button_kind(state: ButtonState) -> MouseReportKind {
    match state {
        ButtonState::Pressed => MouseReportKind::Press,
        ButtonState::Released => MouseReportKind::Release,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::focus::MouseDisabled;
    use crate::input::mouse::test_support::{
        CapturedEffects, add_effect_capture_observers, set_phys_cursor, test_metrics,
    };
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::message::Messages;
    use bevy::ui::{ComputedNode, UiGlobalTransform};
    use orzma_tty_renderer::schema::TerminalGrid;

    fn make_selection_app() -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<OrzmaMouseConfig>()
            .init_resource::<OrzmaMouseGesture>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<CapturedEffects>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_buttons);
        add_effect_capture_observers(&mut app);

        app.world_mut().spawn((
            OrzmaTerminal,
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
            .resource_mut::<Messages<CursorMoved>>()
            .write(CursorMoved {
                window: Entity::PLACEHOLDER,
                position: pos,
                delta: None,
            });
    }

    fn write_left(app: &mut App, state: ButtonState) {
        app.world_mut()
            .resource_mut::<Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    fn ev(kind: MouseReportKind, col: u32, row: u32, count: u8) -> ButtonEvent {
        ButtonEvent {
            kind,
            button: MouseButtonKind::Left,
            cell: CellCoord { col, row },
            side: CellSide::Left,
            click_count: count,
        }
    }

    /// Asserts that a live cursor always wins, an off-window cursor falls
    /// back to the last known position only while a gesture is active, and
    /// an idle off-window cursor yields nothing.
    ///
    /// Case: the user drags a selection out of the window and back, or
    /// simply parks the pointer outside the window with no button held.
    #[test]
    fn effective_drag_cursor_truth_table() {
        let live = Vec2::new(10.0, 10.0);
        let last = Vec2::new(99.0, 88.0);
        assert_eq!(
            effective_drag_cursor(Some(live), false, Some(last)),
            Some(live)
        );
        assert_eq!(
            effective_drag_cursor(Some(live), true, Some(last)),
            Some(live)
        );
        assert_eq!(effective_drag_cursor(None, true, Some(last)), Some(last));
        assert_eq!(effective_drag_cursor(None, true, None), None);
        assert_eq!(effective_drag_cursor(None, false, Some(last)), None);
    }

    /// Asserts that a drag in progress keeps its held pointer and started
    /// selection when the cursor leaves the window, and pins the selection
    /// to the rightmost column while outside.
    ///
    /// Case: the user drags a selection past the window's right edge.
    #[test]
    fn drag_survives_cursor_leaving_window() {
        let mut app = make_selection_app();

        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();

        set_phys_cursor(&mut app, Vec2::new(80.0, 48.0));
        app.update();
        assert!(
            matches!(
                app.world().resource::<OrzmaMouseGesture>().drag,
                Some(DragGesture {
                    phase: DragPhase::Started,
                    ..
                })
            ),
            "dragging across a cell must start the selection"
        );

        app.world_mut().resource_mut::<CapturedEffects>().0.clear();
        set_phys_cursor(&mut app, Vec2::new(900.0, 700.0));
        write_cursor_moved(&mut app, Vec2::new(900.0, 700.0));
        app.update();

        let g = app.world().resource::<OrzmaMouseGesture>();
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

    /// Asserts that releasing the button while the cursor is outside the
    /// window still copies the selection and ends the gesture.
    ///
    /// Case: the user drags a selection out of the window and lets go of
    /// the button there.
    #[test]
    fn release_after_leaving_window_copies() {
        let mut app = make_selection_app();

        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();
        set_phys_cursor(&mut app, Vec2::new(80.0, 48.0));
        app.update();

        set_phys_cursor(&mut app, Vec2::new(900.0, 700.0));
        write_cursor_moved(&mut app, Vec2::new(900.0, 700.0));
        app.update();

        app.world_mut().resource_mut::<CapturedEffects>().0.clear();
        write_left(&mut app, ButtonState::Released);
        app.update();

        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().any(|e| matches!(e, MouseEffect::Copy)),
            "releasing after leaving the window must copy the selection, got {:?}",
            cap.0
        );
        let g = app.world().resource::<OrzmaMouseGesture>();
        assert!(
            g.held.is_none() && g.drag.is_none(),
            "release must end the gesture"
        );
    }

    /// Asserts that cursor motion outside the window with no button held
    /// leaves the gesture reset and clears the remembered cursor position.
    ///
    /// Case: the user moves the pointer across the desktop past the
    /// terminal window without pressing anything.
    #[test]
    fn idle_cursor_outside_window_resets_and_clears_last() {
        let mut app = make_selection_app();

        set_phys_cursor(&mut app, Vec2::new(900.0, 700.0));
        write_cursor_moved(&mut app, Vec2::new(900.0, 700.0));
        app.update();

        let g = app.world().resource::<OrzmaMouseGesture>();
        assert!(
            g.drag.is_none() && g.held.is_none(),
            "an idle frame with no in-window cursor must stay reset"
        );
        assert!(
            g.last_cursor_phys.is_none(),
            "the idle reset must clear last_cursor_phys"
        );
    }

    /// Asserts that a press over a `MouseDisabled` terminal is drained
    /// without arming a drag.
    ///
    /// Case: the user clicks a terminal whose mouse input is disabled
    /// because it is in vi mode.
    #[test]
    fn mouse_disabled_terminal_drains_without_arming_a_gesture() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<OrzmaMouseConfig>()
            .init_resource::<OrzmaMouseGesture>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_buttons);
        app.world_mut().spawn((OrzmaTerminal, MouseDisabled));
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .resource_mut::<Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert!(app.world().resource::<OrzmaMouseGesture>().drag.is_none());
    }

    /// Asserts that a single left press arms a drag, clears any existing
    /// selection without starting a new one, and triggers `PaneClicked`
    /// exactly once so click-to-focus can react to it.
    ///
    /// Case: the user clicks once on a cell with no modifier held.
    #[test]
    fn local_single_press_arms_drag_and_clears() {
        #[derive(Resource, Default)]
        struct Clicks(u32);

        let mut app = make_selection_app();
        app.init_resource::<Clicks>()
            .add_observer(|_ev: On<PaneClicked>, mut clicks: ResMut<Clicks>| clicks.0 += 1);

        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();

        assert_eq!(
            app.world().resource::<Clicks>().0,
            1,
            "a press must trigger PaneClicked exactly once"
        );
        assert_eq!(
            app.world().resource::<CapturedEffects>().0,
            vec![MouseEffect::SelClear]
        );
        assert!(matches!(
            app.world().resource::<OrzmaMouseGesture>().drag,
            Some(DragGesture {
                phase: DragPhase::Armed,
                ..
            })
        ));
    }

    /// Asserts that the first drag off the armed cell starts the selection
    /// at the origin and extends it, and later drags only extend.
    ///
    /// Case: the user presses on a cell and drags across two more cells.
    #[test]
    fn local_drag_materializes_then_extends() {
        let mut g = OrzmaMouseGesture::default();
        decide_button(
            &mut g,
            ev(MouseReportKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        let fx = decide_button(
            &mut g,
            ev(MouseReportKind::Drag, 7, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        assert_eq!(
            fx,
            vec![
                MouseEffect::SelStart {
                    point: to_grid_point(CellCoord { col: 5, row: 5 }),
                    side: CellSide::Left,
                    ty: SelectionKind::Simple
                },
                MouseEffect::SelUpdate {
                    point: to_grid_point(CellCoord { col: 7, row: 5 }),
                    side: CellSide::Left
                },
            ]
        );
        let fx2 = decide_button(
            &mut g,
            ev(MouseReportKind::Drag, 9, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        assert_eq!(
            fx2,
            vec![MouseEffect::SelUpdate {
                point: to_grid_point(CellCoord { col: 9, row: 5 }),
                side: CellSide::Left
            }]
        );
    }

    /// Asserts that releasing after a started drag copies the selection
    /// and ends the drag.
    ///
    /// Case: the user finishes a drag selection and lets go of the button.
    #[test]
    fn release_after_drag_copies() {
        let mut g = OrzmaMouseGesture::default();
        decide_button(
            &mut g,
            ev(MouseReportKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        decide_button(
            &mut g,
            ev(MouseReportKind::Drag, 7, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        let fx = decide_button(
            &mut g,
            ev(MouseReportKind::Release, 7, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        assert_eq!(fx, vec![MouseEffect::Copy]);
        assert!(g.drag.is_none());
    }

    /// Asserts that releasing on the armed cell without dragging copies
    /// nothing and ends the drag.
    ///
    /// Case: the user clicks a cell and releases without moving.
    #[test]
    fn release_after_bare_click_does_not_copy() {
        let mut g = OrzmaMouseGesture::default();
        decide_button(
            &mut g,
            ev(MouseReportKind::Press, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        let fx = decide_button(
            &mut g,
            ev(MouseReportKind::Release, 5, 5, 1),
            ProtocolModifiers::default(),
            false,
            None,
        );
        assert_eq!(fx, vec![]);
        assert!(g.drag.is_none());
    }

    /// Asserts a double-click starts a selection immediately (no arm-then-drag
    /// defer) using `SelectionKind::Simple` — `orzma_vt` has no word-snapped
    /// Semantic kind yet, so double-click rounds down to plain Simple (D16).
    ///
    /// Case: the user double-clicks a word, expecting an immediate selection
    /// rather than the single-click arm-and-defer behavior.
    #[test]
    fn double_click_starts_selection_immediately_pending_semantic_kind() {
        let mut g = OrzmaMouseGesture::default();
        let fx = decide_button(
            &mut g,
            ev(MouseReportKind::Press, 5, 5, 2),
            ProtocolModifiers::default(),
            false,
            None,
        );
        assert_eq!(
            fx,
            vec![MouseEffect::SelStart {
                point: to_grid_point(CellCoord { col: 5, row: 5 }),
                side: CellSide::Left,
                ty: SelectionKind::Simple
            }]
        );
    }

    /// Asserts that a modifier-click on a linked cell opens the URI and
    /// arms no drag.
    ///
    /// Case: the user Cmd-clicks an OSC 8 hyperlink in the terminal.
    #[test]
    fn cmd_click_on_link_opens_and_consumes() {
        let mut g = OrzmaMouseGesture::default();
        let fx = decide_button(
            &mut g,
            ev(MouseReportKind::Press, 5, 5, 1),
            ProtocolModifiers {
                meta: true,
                ..Default::default()
            },
            true,
            Some("https://example.com".into()),
        );
        assert_eq!(fx, vec![MouseEffect::OpenUri("https://example.com".into())]);
        assert!(g.drag.is_none(), "a link-open press must not arm a drag");
    }

    /// Asserts that `to_grid_point` turns a 1-indexed protocol cell into a
    /// 0-indexed viewport point.
    ///
    /// Case: a press lands on the third row, fifth column of the pane.
    #[test]
    fn to_grid_point_zero_indexes_the_one_indexed_cell() {
        let p = to_grid_point(CellCoord { col: 5, row: 3 });
        assert_eq!(p.line.0, 2);
        assert_eq!(p.column.0, 4);
    }

    /// Asserts that `protocol_mods` reports Ctrl and Shift from the key
    /// state and leaves Alt and Meta clear.
    ///
    /// Case: the user holds Ctrl+Shift while clicking.
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
