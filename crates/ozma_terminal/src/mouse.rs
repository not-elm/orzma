//! Default mouse handler for the Ozma terminal: app reporting, local text
//! selection + copy, wheel scrollback, and Cmd-click hyperlink open. Reads Bevy
//! mouse input, hit-tests the cursor to a cell, and drives the engine's pure
//! `ButtonAction` / `WheelAction` routers, applying the result to the
//! `TerminalHandle` / `Clipboard`. Gated per entity by `InputDisabled`.

use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::time::{Real, Time};
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use ozma_tty_engine::{
    ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, CellCoord, Coalescer, Column, Line,
    MouseButtonKind, Point, ProtocolModifiers, PtyHandle, SelectionType, Side, TermMode,
    TerminalHandle, TerminalModifiers, WheelAction, WheelConfig, WheelModifiers,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::TerminalGrid;
use std::time::Duration;

use crate::clipboard::Clipboard;
use crate::hyperlink::{hyperlink_hover_cursor, link_modifier_held, try_open_uri};
use crate::input::{InputDisabled, current_terminal_modifiers};
use crate::spawn::OzmaTerminal;

/// Which modifier activates "fine" (1 line per notch) wheel scrolling.
/// Crate-local mirror of the host config enum (the crate must not depend on
/// `ozmux_configs`). Default `Alt`: on macOS Shift+wheel becomes horizontal
/// scroll at the OS level, so Shift never reaches the app as vertical `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FineModifier {
    /// Shift key activates fine scrolling.
    Shift,
    /// Control key activates fine scrolling.
    Ctrl,
    /// Alt/Option key activates fine scrolling.
    #[default]
    Alt,
    /// Fine scrolling is disabled (always coarse).
    None,
}

/// Host-supplied mouse policy. `Default` is a working spawn-and-go config; the
/// host overrides it from `ozmux_configs`.
#[derive(Resource)]
pub struct OzmaMouseConfig {
    /// Button-report burst cap. MUST be non-zero or forwarded clicks are dropped.
    pub buttons: ButtonConfig,
    /// Wheel routing config (lines-per-notch, fine lines, burst cap).
    pub wheel: WheelConfig,
    /// Cells of wheel travel per emitted notch (smooth-scroll accumulation).
    pub cells_per_notch: f32,
    /// Max gap between clicks counted as a double / triple click.
    pub double_click_timeout: Duration,
    /// Max cursor drift (logical px) between clicks of one chord.
    pub click_drift_px: f32,
    /// Which modifier activates fine scrolling.
    pub fine_modifier: FineModifier,
}

impl Default for OzmaMouseConfig {
    fn default() -> Self {
        Self {
            buttons: ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
            wheel: WheelConfig::default(),
            cells_per_notch: 0.5,
            double_click_timeout: Duration::from_millis(400),
            click_drift_px: 8.0,
            fine_modifier: FineModifier::Alt,
        }
    }
}

/// System set for the crate's three mouse systems. Hosts maintaining
/// `InputDisabled` should schedule their maintainer `.before(OzmaTerminalMouseSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct OzmaTerminalMouseSet;

/// Phase of an in-progress left-drag: `Armed` after a single-click press (no
/// selection started yet), `Started` once the pointer crossed into another cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragPhase {
    /// The button is held but the pointer has not left the origin cell.
    Armed,
    /// The pointer has crossed a cell boundary and selection is active.
    Started,
}

/// An in-progress button gesture: the held button, the selection anchor, and the
/// last cell a drag reached (dedup + lazy materialization).
pub(crate) struct DragGesture {
    /// Which mouse button is being held.
    pub(crate) button: MouseButtonKind,
    /// The cell where the gesture originated.
    pub(crate) origin: CellCoord,
    /// The half of the origin cell where the gesture started.
    pub(crate) side: Side,
    /// The selection granularity (word, line, etc.).
    pub(crate) ty: SelectionType,
    /// Current phase of the drag.
    pub(crate) phase: DragPhase,
    /// The last cell the drag endpoint was updated to (for dedup).
    pub(crate) last_cell: CellCoord,
}

/// Tracks the current mouse gesture and consecutive-click count.
#[derive(Resource, Default)]
pub(crate) struct OzmaMouseGesture {
    /// Consecutive-click counter for multi-click detection.
    pub(crate) click: ClickTracker,
    /// In-progress button gesture, or `None` when idle.
    pub(crate) drag: Option<DragGesture>,
}

/// Consecutive-click counter using a timeout + positional-drift gate.
#[derive(Default)]
pub(crate) struct ClickTracker {
    last: Option<(Duration, Vec2, u8)>,
}

impl ClickTracker {
    /// Registers a press at `now` / logical `pos`, returning the click count
    /// (1..=3). `cfg` is `(timeout, drift_px)`.
    pub(crate) fn register(&mut self, now: Duration, pos: Vec2, cfg: (Duration, f32)) -> u8 {
        let (timeout, drift) = cfg;
        let count = match self.last {
            Some((t, p, c)) if now.saturating_sub(t) <= timeout && p.distance(pos) <= drift => {
                (c + 1).min(3)
            }
            _ => 1,
        };
        self.last = Some((now, pos, count));
        count
    }
}

/// 1-indexed `(CellCoord, Side)` of the cell at pane-local physical `local`,
/// clamped to `1..=cols` × `1..=rows`. `Side` is `Left` in the left half.
pub(crate) fn cell_at_local(
    local: Vec2,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
) -> (CellCoord, Side) {
    let col_f = (local.x / cell_w).max(0.0);
    let row_f = (local.y / cell_h).max(0.0);
    let col = (col_f.floor() as u32 + 1).min(cols as u32).max(1);
    let row = (row_f.floor() as u32 + 1).min(rows as u32).max(1);
    let side = if col_f - col_f.floor() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (CellCoord { col, row }, side)
}

/// Resolves the window-space physical cursor to a cell on the terminal node, or
/// `None` when the cursor is outside the node.
pub(crate) fn cell_at_cursor(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
) -> Option<(CellCoord, Side)> {
    let local = node
        .normalize_point(*transform, cursor_phys)
        .map(|n| (n + Vec2::splat(0.5)) * node.size)?;
    Some(cell_at_local(local, cell_w, cell_h, cols, rows))
}

/// Converts a 1-indexed protocol `CellCoord` into the engine's viewport-relative
/// selection `Point` (row 0 = top of viewport; the engine translates for scroll).
pub(crate) fn to_viewport_point(cell: CellCoord) -> Point {
    Point::new(Line(cell.row as i32 - 1), Column(cell.col as usize - 1))
}

/// Builds `ProtocolModifiers` from the held keys.
pub(crate) fn protocol_mods(keys: &ButtonInput<KeyCode>) -> ProtocolModifiers {
    let m = current_terminal_modifiers(keys);
    ProtocolModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
    }
}

/// A resolved intent the apply step writes to the handle / clipboard. Kept
/// separate from application so the decision logic is unit-testable without a
/// `TerminalHandle` (which has no public constructor).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MouseEffect {
    /// Write these bytes to the PTY.
    Write(Vec<u8>),
    /// Start a new local selection at `point`.
    SelStart { point: Point, side: Side, ty: SelectionType },
    /// Extend the current selection's moving end to `point`.
    SelUpdate { point: Point, side: Side },
    /// Clear any active local selection.
    SelClear,
    /// Copy the current selection to the clipboard.
    Copy,
    /// Scroll the viewport by `i32` lines (negative = up).
    Scroll(i32),
    /// Open the given URI in the host browser / handler.
    OpenUri(String),
}

/// Pure per-event decision for a mouse button. Mutates `gesture` (drag phase /
/// click state) and returns the effects to apply. A Cmd/Ctrl-click on a linked
/// cell opens the URL and consumes the event; otherwise the engine's
/// `ButtonAction::route` decides forward-to-app vs local selection.
pub(crate) fn decide_button(
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
                button: evt.button,
                origin: cell,
                side,
                ty,
                phase: DragPhase::Armed,
                last_cell: cell,
            });
            vec![MouseEffect::SelClear]
        }
        ButtonAction::StartLocalSelection { ty, cell, side } => {
            gesture.drag = Some(DragGesture {
                button: evt.button,
                origin: cell,
                side,
                ty,
                phase: DragPhase::Started,
                last_cell: cell,
            });
            vec![MouseEffect::SelStart { point: to_viewport_point(cell), side, ty }]
        }
        ButtonAction::UpdateLocalSelection { cell, side } => update_selection(gesture, cell, side),
        ButtonAction::ClearLocalSelection => {
            gesture.drag = None;
            vec![MouseEffect::SelClear]
        }
    };

    if evt.kind == ButtonEventKind::Release && evt.button == MouseButtonKind::Left {
        if effects.is_empty()
            && matches!(&gesture.drag, Some(d) if d.phase == DragPhase::Started)
        {
            effects.push(MouseEffect::Copy);
        }
        gesture.drag = None;
    }
    effects
}

/// Carries the sub-notch wheel remainder across frames.
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_cells: f32,
}

/// Cells of scroll for one wheel event: `Line` units count directly, `Pixel`
/// units divide by the cell height. Positive = wheel-up (toward older lines).
pub(crate) fn wheel_delta_cells(unit: MouseScrollUnit, y: f32, cell_h: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => y,
        MouseScrollUnit::Pixel => y / cell_h.max(1.0),
    }
}

/// Adds `delta_cells` to the accumulator and returns whole notches to emit
/// (positive = up/older), carrying the remainder. Resets the residual on a
/// sign flip, then processes the new delta at full magnitude.
pub(crate) fn accumulate_notches(
    acc: &mut WheelAccumulator,
    delta_cells: f32,
    cells_per_notch: f32,
) -> i32 {
    if acc.residual_cells != 0.0 && acc.residual_cells.signum() != delta_cells.signum() {
        acc.residual_cells = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    acc.residual_cells += delta_cells;
    let notches = (acc.residual_cells / threshold).trunc() as i32;
    if notches != 0 {
        acc.residual_cells -= notches as f32 * threshold;
    }
    notches
}

/// Pure wheel decision. `notches` is in the engine convention (negative =
/// up/older); callers negate the Bevy-derived up-positive value before calling.
pub(crate) fn decide_wheel(
    modes: TermMode,
    notches: i32,
    cell: CellCoord,
    mods: WheelModifiers,
    cfg: &WheelConfig,
) -> Vec<MouseEffect> {
    match WheelAction::route(modes, notches, cell, mods, cfg) {
        WheelAction::Noop => Vec::new(),
        WheelAction::WriteToPty(b) => vec![MouseEffect::Write(b)],
        WheelAction::ScrollViewport(lines) => vec![MouseEffect::Scroll(lines)],
    }
}

/// The crate's mouse-button dispatcher. Resolves the cursor cell, tracks clicks
/// and drag state, drives `decide_button`, and applies the effects. Skips the
/// `OzmaTerminal` while it carries `InputDisabled`.
pub(crate) fn dispatch_mouse_buttons(
    mut gesture: ResMut<OzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut terminal: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    mut clipboard: ResMut<Clipboard>,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((mut handle, mut pty, mut coalescer, node, transform, grid)) = terminal.single_mut() else {
        buttons.clear();
        gesture.drag = None;
        return;
    };
    let Ok(window) = windows.single() else {
        buttons.clear();
        gesture.drag = None;
        return;
    };
    if !window.focused {
        buttons.clear();
        gesture.drag = None;
        return;
    }
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        buttons.clear();
        return;
    };
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let modes = handle.current_modes();
    let mods = protocol_mods(&keys);
    let modifier_held = link_modifier_held(&mods);

    for ev in buttons.read() {
        let Some(button) = map_button(ev.button) else { continue };
        let Some((cell, side)) = cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, grid.cols, grid.rows) else { continue };
        let kind = match ev.state {
            ButtonState::Pressed => ButtonEventKind::Press,
            ButtonState::Released => ButtonEventKind::Release,
        };
        let click_count = if kind == ButtonEventKind::Press {
            gesture.click.register(time.elapsed(), cursor_phys / scale, (cfg.double_click_timeout, cfg.click_drift_px))
        } else {
            1
        };
        let link_at_cell = (kind == ButtonEventKind::Press && button == MouseButtonKind::Left && modifier_held)
            .then(|| grid.hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16).map(|(_id, uri)| uri.as_str().to_string()))
            .flatten();
        let evt = ButtonEvent { kind, button, cell, side, click_count };
        let effects = decide_button(&mut gesture, modes, evt, mods, modifier_held, link_at_cell, &cfg.buttons);
        for effect in effects {
            apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
        }
    }

    if gesture.drag.is_some()
        && let Some((cell, side)) = cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, grid.cols, grid.rows)
        && gesture.drag.as_ref().is_some_and(|d| d.last_cell != cell)
    {
        let button = gesture.drag.as_ref().map(|d| d.button).unwrap_or(MouseButtonKind::Left);
        let evt = ButtonEvent { kind: ButtonEventKind::Drag, button, cell, side, click_count: 1 };
        let effects = decide_button(&mut gesture, modes, evt, mods, modifier_held, None, &cfg.buttons);
        for effect in effects {
            apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
        }
    }
}

/// The crate's wheel dispatcher: accumulates notches, drives `decide_wheel`,
/// applies the result.
pub(crate) fn dispatch_mouse_wheel(
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    mut terminal: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    mut clipboard: ResMut<Clipboard>,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((mut handle, mut pty, mut coalescer, node, transform, grid)) = terminal.single_mut() else {
        wheel.clear();
        return;
    };
    let Ok(window) = windows.single() else { wheel.clear(); return };
    if !window.focused { wheel.clear(); return }
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);

    let mut delta_cells = 0.0f32;
    for ev in wheel.read() {
        delta_cells += wheel_delta_cells(ev.unit, ev.y, cell_h);
    }
    let raw = accumulate_notches(&mut gesture_acc, delta_cells, cfg.cells_per_notch);
    if raw == 0 {
        return;
    }
    // NOTE: Bevy +y (up/older) → engine convention (negative = up/older).
    let notches = -raw;
    let cell = window
        .cursor_position()
        .map(|c| c * window.scale_factor())
        .and_then(|p| cell_at_cursor(node, transform, p, cell_w, cell_h, grid.cols, grid.rows))
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    let m = current_terminal_modifiers(&keys);
    let mods = WheelModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        fine: fine_held(cfg.fine_modifier, &m),
    };
    for effect in decide_wheel(handle.current_modes(), notches, cell, mods, &cfg.wheel) {
        apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
    }
}

fn fine_held(modifier: FineModifier, m: &TerminalModifiers) -> bool {
    match modifier {
        FineModifier::Shift => m.shift,
        FineModifier::Ctrl => m.ctrl,
        FineModifier::Alt => m.alt,
        FineModifier::None => true,
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

fn apply_effect(
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    clipboard: &mut Clipboard,
    effect: MouseEffect,
) {
    match effect {
        MouseEffect::Write(b) => {
            if let Err(e) = handle.write(pty, &b) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        }
        MouseEffect::SelStart { point, side, ty } => handle.selection_start_at(coalescer, point, side, ty),
        MouseEffect::SelUpdate { point, side } => handle.selection_update_to(coalescer, point, side),
        MouseEffect::SelClear => handle.selection_clear(coalescer),
        MouseEffect::Copy => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
        }
        MouseEffect::Scroll(lines) => handle.scroll(coalescer, lines),
        MouseEffect::OpenUri(uri) => try_open_uri(&uri),
    }
}

/// Lazily materializes an armed selection on the first cell change, then extends.
fn update_selection(gesture: &mut OzmaMouseGesture, cell: CellCoord, side: Side) -> Vec<MouseEffect> {
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
            drag.last_cell = cell;
            vec![
                MouseEffect::SelStart { point: to_viewport_point(origin), side: origin_side, ty },
                MouseEffect::SelUpdate { point: to_viewport_point(cell), side },
            ]
        }
        DragPhase::Started => {
            drag.last_cell = cell;
            vec![MouseEffect::SelUpdate { point: to_viewport_point(cell), side }]
        }
    }
}

/// Registers the crate's mouse systems and resources.
pub(crate) struct OzmaMousePlugin;

impl Plugin for OzmaMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OzmaMouseConfig>()
            .init_resource::<OzmaMouseGesture>()
            .init_resource::<WheelAccumulator>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .add_systems(
                Update,
                hyperlink_hover_cursor
                    .in_set(OzmaTerminalMouseSet)
                    .run_if(on_message::<KeyboardInput>.or(on_message::<CursorMoved>)),
            )
            .add_systems(
                Update,
                (dispatch_mouse_buttons, dispatch_mouse_wheel)
                    .in_set(OzmaTerminalMouseSet)
                    .run_if(on_message::<MouseButtonInput>.or(on_message::<CursorMoved>).or(on_message::<MouseWheel>)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::mouse::{MouseButton, MouseButtonInput};
    use ozma_tty_engine::{ButtonEvent, ButtonEventKind, MouseButtonKind};

    #[test]
    fn input_disabled_terminal_drains_without_arming_a_gesture() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .init_resource::<OzmaMouseConfig>()
            .init_resource::<OzmaMouseGesture>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Clipboard>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_buttons);
        app.world_mut().spawn((OzmaTerminal, InputDisabled));
        app.world_mut().spawn((Window { focused: true, ..default() }, PrimaryWindow));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput { button: MouseButton::Left, state: ButtonState::Pressed, window: Entity::PLACEHOLDER });
        app.update();
        assert!(app.world().resource::<OzmaMouseGesture>().drag.is_none());
    }

    fn test_metrics() -> TerminalCellMetricsResource {
        use ozma_tty_renderer::CellMetrics;
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0, line_height_phys: 16.0, ascent_phys: 12.0, descent_phys: 4.0,
                underline_position_phys: -2.0, underline_thickness_phys: 1.0, max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        }
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
    fn local_single_press_arms_drag_and_clears() {
        let mut g = OzmaMouseGesture::default();
        let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers::default(), false, None, &ButtonConfig { max_protocol_events_per_frame: 8 });
        assert_eq!(fx, vec![MouseEffect::SelClear]);
        assert!(matches!(g.drag, Some(DragGesture { phase: DragPhase::Armed, .. })));
    }

    #[test]
    fn local_drag_materializes_then_extends() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
        decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Drag, 7, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        assert_eq!(fx, vec![
            MouseEffect::SelStart { point: to_viewport_point(CellCoord { col: 5, row: 5 }), side: Side::Left, ty: SelectionType::Simple },
            MouseEffect::SelUpdate { point: to_viewport_point(CellCoord { col: 7, row: 5 }), side: Side::Left },
        ]);
        let fx2 = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Drag, 9, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        assert_eq!(fx2, vec![MouseEffect::SelUpdate { point: to_viewport_point(CellCoord { col: 9, row: 5 }), side: Side::Left }]);
    }

    #[test]
    fn release_after_drag_copies() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
        decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Drag, 7, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Release, 7, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        assert_eq!(fx, vec![MouseEffect::Copy]);
        assert!(g.drag.is_none());
    }

    #[test]
    fn release_after_bare_click_does_not_copy() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
        decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Release, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
        assert_eq!(fx, vec![]);
        assert!(g.drag.is_none());
    }

    #[test]
    fn double_click_starts_word_selection() {
        let mut g = OzmaMouseGesture::default();
        let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
        let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 2), ProtocolModifiers::default(), false, None, &cfg);
        assert_eq!(fx, vec![MouseEffect::SelStart { point: to_viewport_point(CellCoord { col: 5, row: 5 }), side: Side::Left, ty: SelectionType::Semantic }]);
    }

    #[test]
    fn app_capture_press_forwards_sgr_bytes() {
        let mut g = OzmaMouseGesture::default();
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let fx = decide_button(&mut g, modes, ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &ButtonConfig { max_protocol_events_per_frame: 8 });
        assert_eq!(fx, vec![MouseEffect::SelClear, MouseEffect::Write(b"\x1b[<0;5;5M".to_vec())]);
    }

    #[test]
    fn shift_bypass_selects_even_when_captured() {
        let mut g = OzmaMouseGesture::default();
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let mods = ProtocolModifiers { shift: true, ..Default::default() };
        let fx = decide_button(&mut g, modes, ev(ButtonEventKind::Press, 5, 5, 1), mods, false, None, &ButtonConfig { max_protocol_events_per_frame: 8 });
        assert_eq!(fx, vec![MouseEffect::SelClear]);
        assert!(matches!(g.drag, Some(DragGesture { phase: DragPhase::Armed, .. })));
    }

    #[test]
    fn cmd_click_on_link_opens_and_consumes() {
        let mut g = OzmaMouseGesture::default();
        let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1),
            ProtocolModifiers { meta: true, ..Default::default() }, true, Some("https://example.com".into()),
            &ButtonConfig { max_protocol_events_per_frame: 8 });
        assert_eq!(fx, vec![MouseEffect::OpenUri("https://example.com".into())]);
        assert!(g.drag.is_none(), "a link-open press must not arm a drag");
    }

    #[test]
    fn default_config_sets_button_cap_explicitly() {
        let cfg = OzmaMouseConfig::default();
        assert_eq!(cfg.buttons.max_protocol_events_per_frame, 8, "must NOT be ButtonConfig::default()'s 0");
        assert_eq!(cfg.wheel.max_protocol_events_per_frame, 8);
        assert_eq!(cfg.cells_per_notch, 0.5);
        assert_eq!(cfg.double_click_timeout, std::time::Duration::from_millis(400));
        assert_eq!(cfg.click_drift_px, 8.0);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
    }

    #[test]
    fn cell_at_local_is_one_indexed_and_clamped() {
        let (cell, side) = cell_at_local(Vec2::new(0.0, 0.0), 10.0, 20.0, 80, 24);
        assert_eq!((cell.col, cell.row), (1, 1));
        assert_eq!(side, Side::Left);
        let (cell, _) = cell_at_local(Vec2::new(10_000.0, 10_000.0), 10.0, 20.0, 80, 24);
        assert_eq!((cell.col, cell.row), (80, 24));
        let (cell, side) = cell_at_local(Vec2::new(17.0, 5.0), 10.0, 20.0, 80, 24);
        assert_eq!(cell.col, 2);
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn to_viewport_point_zero_indexes_the_one_indexed_cell() {
        let p = to_viewport_point(CellCoord { col: 5, row: 3 });
        assert_eq!(p.line.0, 2);
        assert_eq!(p.column.0, 4);
    }

    #[test]
    fn click_tracker_counts_within_timeout_and_drift() {
        let mut t = ClickTracker::default();
        let cfg = (std::time::Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(std::time::Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(std::time::Duration::from_millis(200), Vec2::new(11.0, 11.0), cfg), 2);
        assert_eq!(t.register(std::time::Duration::from_millis(350), Vec2::new(12.0, 10.0), cfg), 3);
        assert_eq!(t.register(std::time::Duration::from_millis(900), Vec2::new(12.0, 10.0), cfg), 1);
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

    #[test]
    fn cell_at_cursor_resolves_known_point() {
        let node = ComputedNode {
            size: Vec2::new(800.0, 600.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(400.0, 300.0);
        // node center is (400, 300), so physical (0, 0) is top-left, (800, 600) is bottom-right
        // at cell pitch 10x20 with 80 cols, 30 rows:
        // physical (15, 25) → local (15, 25) → col 2 (10-19), row 2 (20-39)
        let result = cell_at_cursor(&node, &transform, Vec2::new(15.0, 25.0), 10.0, 20.0, 80, 30);
        let (cell, _) = result.expect("point inside node must resolve");
        assert_eq!(cell.col, 2);
        assert_eq!(cell.row, 2);
    }

    #[test]
    fn drag_gesture_phase_transitions() {
        let armed = DragGesture {
            button: MouseButtonKind::Left,
            origin: CellCoord { col: 1, row: 1 },
            side: Side::Left,
            ty: SelectionType::Simple,
            phase: DragPhase::Armed,
            last_cell: CellCoord { col: 1, row: 1 },
        };
        assert_eq!(armed.phase, DragPhase::Armed);
        assert_eq!(armed.button, MouseButtonKind::Left);
        assert_eq!((armed.origin.col, armed.origin.row), (1, 1));
        assert_eq!(armed.side, Side::Left);
        assert_eq!(armed.ty, SelectionType::Simple);
        assert_eq!((armed.last_cell.col, armed.last_cell.row), (1, 1));

        let started = DragGesture {
            phase: DragPhase::Started,
            last_cell: CellCoord { col: 3, row: 2 },
            ..armed
        };
        assert_eq!(started.phase, DragPhase::Started);
        assert_eq!((started.last_cell.col, started.last_cell.row), (3, 2));
    }

    #[test]
    fn mouse_gesture_resource_default_is_idle() {
        let g = OzmaMouseGesture::default();
        assert!(g.drag.is_none());
        let mut t = g.click;
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::ZERO, cfg), 1);
    }

    #[test]
    fn line_delta_is_direct_pixel_divides_by_cell_height() {
        assert_eq!(wheel_delta_cells(MouseScrollUnit::Line, 2.0, 16.0), 2.0);
        assert_eq!(wheel_delta_cells(MouseScrollUnit::Pixel, 32.0, 16.0), 2.0);
    }

    #[test]
    fn accumulator_emits_on_threshold_and_carries_remainder() {
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 1);
        assert_eq!(accumulate_notches(&mut acc, -1.0, 0.5), -2);
    }

    #[test]
    fn scrollback_up_returns_positive_viewport_scroll() {
        // Bevy +y (wheel up) → caller negates → engine notches negative → into history.
        let fx = decide_wheel(TermMode::empty(), -1, CellCoord { col: 1, row: 1 }, WheelModifiers::default(), &WheelConfig::default());
        assert_eq!(fx, vec![MouseEffect::Scroll(3)]);
    }

    #[test]
    fn app_capture_wheel_forwards_bytes() {
        let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let fx = decide_wheel(modes, -1, CellCoord { col: 1, row: 1 }, WheelModifiers::default(), &WheelConfig::default());
        assert!(matches!(fx.as_slice(), [MouseEffect::Write(b)] if !b.is_empty()));
    }
}
