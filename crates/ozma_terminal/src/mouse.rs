//! Default mouse handler for the Ozma terminal: app reporting, local text
//! selection + copy, wheel scrollback, and Cmd-click hyperlink open. Reads Bevy
//! mouse input, hit-tests the cursor to a cell, and drives the engine's pure
//! `ButtonAction` / `WheelAction` routers, applying the result to the
//! `TerminalHandle` / `Clipboard`. Gated per entity by `MouseDisabled`.

use bevy::input::ButtonState;
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
use crate::hyperlink::{link_modifier_held, try_open_uri};
use crate::input::current_terminal_modifiers;
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
    /// No modifier required; fine scrolling is always active.
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
/// `MouseDisabled` should schedule their maintainer `.before(OzmaTerminalMouseSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct OzmaTerminalMouseSet;

/// When present on an `OzmaTerminal` entity, the crate's mouse dispatchers and
/// hover-cursor system skip it — it is removed from the hit-test candidate set,
/// so the pointer falls through to the next terminal below it. The host marks
/// every terminal `MouseDisabled` for modal suppression (picker / IME / focused
/// webview / unfocused window).
#[derive(Component)]
pub struct MouseDisabled;

/// Phase of an in-progress left-drag: `Armed` after a single-click press (no
/// selection started yet), `Started` once the pointer crossed into another cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragPhase {
    /// The button is held but the pointer has not left the origin cell.
    Armed,
    /// The pointer has crossed a cell boundary and selection is active.
    Started,
}

/// An in-progress local-selection gesture: the selection anchor, the granularity,
/// and the drag phase (drives lazy materialization of the selection).
pub(crate) struct DragGesture {
    /// The cell where the gesture originated.
    pub(crate) origin: CellCoord,
    /// The half of the origin cell where the gesture started.
    pub(crate) side: Side,
    /// The selection granularity (word, line, etc.).
    pub(crate) ty: SelectionType,
    /// Current phase of the drag.
    pub(crate) phase: DragPhase,
}

/// A held mouse button: the terminal the press landed on, the button, and the
/// last cell a drag was synthesized for. The `entity` locks drag/release to the
/// press terminal even when the pointer wanders onto another terminal. Tracked
/// for BOTH local selection and app-forward drags — the forward path never sets
/// `drag`, so drag-motion synthesis must not depend on it.
#[derive(Clone, Copy)]
pub(crate) struct HeldPointer {
    pub(crate) entity: Entity,
    pub(crate) button: MouseButtonKind,
    pub(crate) last_cell: CellCoord,
}

/// Tracks the current mouse gesture and consecutive-click count.
#[derive(Resource, Default)]
pub(crate) struct OzmaMouseGesture {
    /// Consecutive-click counter for multi-click detection.
    pub(crate) click: ClickTracker,
    /// In-progress local-selection gesture, or `None` when idle.
    pub(crate) drag: Option<DragGesture>,
    /// Held button + last drag cell, for both local and app-forward drags.
    pub(crate) held: Option<HeldPointer>,
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

/// The `Entity` of the topmost `OzmaTerminal` whose node contains `cursor_phys`,
/// or `None` when the cursor is over none. "Topmost" is the highest
/// `ComputedNode::stack_index` (Bevy's resolved front-to-back UI order); a higher
/// index is drawn later, i.e. on top. Equal stack indices (only possible before
/// the first layout pass assigns them) are broken by `Entity` order so the
/// result is deterministic rather than query-iteration dependent.
pub(crate) fn topmost_terminal_at<'a>(
    cursor_phys: Vec2,
    candidates: impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)>,
) -> Option<Entity> {
    candidates
        .filter(|&(_, node, transform)| node.contains_point(*transform, cursor_phys))
        .max_by_key(|&(entity, node, _)| (node.stack_index(), entity))
        .map(|(entity, _, _)| entity)
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
    SelStart {
        point: Point,
        side: Side,
        ty: SelectionType,
    },
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

/// Carries a gather system's decided mouse effects to the apply observer, so the
/// dispatch systems stay read-only on the terminal and all mutation lives in one
/// place (`on_terminal_mouse_effects`), mirroring `PasteAction` / `on_paste`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalMouseEffects {
    /// The terminal entity to apply the effects to.
    #[event_target]
    pub(crate) entity: Entity,
    /// The decided effects, applied in order.
    pub(crate) effects: Vec<MouseEffect>,
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

/// Carries the sub-notch wheel remainder across frames, scoped to the last
/// terminal the wheel targeted.
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_cells: f32,
    last_target: Option<Entity>,
}

impl WheelAccumulator {
    /// Resets the residual when the wheel target changes, so a sub-notch fraction
    /// accumulated over one terminal cannot bleed into the next.
    fn retarget(&mut self, entity: Entity) {
        if self.last_target != Some(entity) {
            self.residual_cells = 0.0;
            self.last_target = Some(entity);
        }
    }
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
    if acc.residual_cells != 0.0
        && delta_cells != 0.0
        && acc.residual_cells.signum() != delta_cells.signum()
    {
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

/// The crate's mouse-button dispatcher. Hit-tests the topmost terminal under the
/// cursor on press, locks drag/release to that terminal, tracks clicks and drag
/// state, drives `decide_button`, and triggers `TerminalMouseEffects`. Skips any
/// `OzmaTerminal` carrying `MouseDisabled`; an empty candidate set (modal
/// suppression) drains events and resets the gesture.
pub(crate) fn dispatch_mouse_buttons(
    mut commands: Commands,
    mut gesture: ResMut<OzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
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
    let Ok(window) = windows.single() else {
        buttons.clear();
        gesture.drag = None;
        gesture.held = None;
        return;
    };
    if !window.focused || terminals.is_empty() {
        buttons.clear();
        gesture.drag = None;
        gesture.held = None;
        return;
    }
    let scale = window.scale_factor();
    let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
        buttons.clear();
        gesture.drag = None;
        gesture.held = None;
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let mods = protocol_mods(&keys);
    let modifier_held = link_modifier_held(&mods);

    for ev in buttons.read() {
        let kind = match ev.state {
            ButtonState::Pressed => ButtonEventKind::Press,
            ButtonState::Released => ButtonEventKind::Release,
        };
        let target = if kind == ButtonEventKind::Press {
            topmost_terminal_at(
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
            gesture.held = None;
            gesture.drag = None;
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
            ButtonEventKind::Release => gesture.held = None,
            _ => {}
        }
        if !decided.is_empty() {
            commands.trigger(TerminalMouseEffects {
                entity: target,
                effects: decided,
            });
        }
    }

    let Some(held) = gesture.held else {
        return;
    };
    let Ok((_, handle, node, transform, grid)) = terminals.get(held.entity) else {
        gesture.held = None;
        gesture.drag = None;
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
        if !drag_effects.is_empty() {
            commands.trigger(TerminalMouseEffects {
                entity: held.entity,
                effects: drag_effects,
            });
        }
    }
}

/// The crate's wheel dispatcher: routes to the topmost terminal under the cursor,
/// resets the accumulator on a target change, accumulates notches, drives
/// `decide_wheel`, and triggers `TerminalMouseEffects`. Skips `MouseDisabled`
/// terminals; an empty candidate set drains the wheel events.
pub(crate) fn dispatch_mouse_wheel(
    mut commands: Commands,
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
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
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        wheel.clear();
        return;
    };
    if !window.focused || terminals.is_empty() {
        wheel.clear();
        return;
    }
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        wheel.clear();
        return;
    };
    let Some(target) = topmost_terminal_at(
        cursor_phys,
        terminals
            .iter()
            .map(|(e, _, node, transform, _)| (e, node, transform)),
    ) else {
        wheel.clear();
        return;
    };
    let Ok((_, handle, node, transform, grid)) = terminals.get(target) else {
        wheel.clear();
        return;
    };
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };

    gesture_acc.retarget(target);
    let delta_cells: f32 = wheel
        .read()
        .map(|ev| wheel_delta_cells(ev.unit, ev.y, ctx.cell_h))
        .sum();
    let raw = accumulate_notches(&mut gesture_acc, delta_cells, cfg.cells_per_notch);
    if raw == 0 {
        return;
    }
    // NOTE: Bevy +y (up/older) → engine convention (negative = up/older).
    let notches = -raw;
    let cell = ctx
        .hit(cursor_phys)
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    let mods = build_wheel_modifiers(&keys, &cfg);
    let effects = decide_wheel(handle.current_modes(), notches, cell, mods, &cfg.wheel);
    if !effects.is_empty() {
        commands.trigger(TerminalMouseEffects {
            entity: target,
            effects,
        });
    }
}

/// Read-only hit-test context for one gather run: the terminal node geometry,
/// cell pitch, and grid dimensions — so helpers resolve a cursor to a cell
/// without re-threading seven arguments.
struct CellContext<'a> {
    node: &'a ComputedNode,
    transform: &'a UiGlobalTransform,
    grid: &'a TerminalGrid,
    cell_w: f32,
    cell_h: f32,
}

impl CellContext<'_> {
    fn hit(&self, cursor_phys: Vec2) -> Option<(CellCoord, Side)> {
        cell_at_cursor(
            self.node,
            self.transform,
            cursor_phys,
            self.cell_w,
            self.cell_h,
            self.grid.cols,
            self.grid.rows,
        )
    }
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

/// Builds `WheelModifiers` from the held keys + the fine-scroll config.
fn build_wheel_modifiers(keys: &ButtonInput<KeyCode>, cfg: &OzmaMouseConfig) -> WheelModifiers {
    let m = current_terminal_modifiers(keys);
    WheelModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        fine: fine_held(cfg.fine_modifier, &m),
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

/// Applies a gather system's decided mouse effects to the target terminal — the
/// sole apply path for both mouse dispatch systems. Runs at command flush (same
/// frame as the trigger), mirroring `on_paste` / `on_terminal_key_input`.
fn on_terminal_mouse_effects(
    ev: On<TerminalMouseEffects>,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer), With<OzmaTerminal>>,
) {
    let Ok((mut handle, mut pty, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    for effect in &ev.effects {
        apply_effect(
            &mut handle,
            &mut pty,
            &mut coalescer,
            &mut clipboard,
            effect,
        );
    }
}

fn apply_effect(
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    clipboard: &mut Clipboard,
    effect: &MouseEffect,
) {
    match effect {
        MouseEffect::Write(b) => {
            if let Err(e) = handle.write(pty, b) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        }
        MouseEffect::SelStart { point, side, ty } => {
            handle.selection_start_at(coalescer, *point, *side, *ty)
        }
        MouseEffect::SelUpdate { point, side } => {
            handle.selection_update_to(coalescer, *point, *side)
        }
        MouseEffect::SelClear => handle.selection_clear(coalescer),
        MouseEffect::Copy => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
        }
        MouseEffect::Scroll(lines) => handle.scroll(coalescer, *lines),
        MouseEffect::OpenUri(uri) => try_open_uri(uri),
    }
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
            .add_observer(on_terminal_mouse_effects)
            .add_systems(
                Update,
                (dispatch_mouse_buttons, dispatch_mouse_wheel)
                    .in_set(OzmaTerminalMouseSet)
                    .run_if(
                        on_message::<MouseButtonInput>
                            .or(on_message::<CursorMoved>)
                            .or(on_message::<MouseWheel>),
                    ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::mouse::{MouseButton, MouseButtonInput};
    use ozma_tty_engine::{ButtonEvent, ButtonEventKind, MouseButtonKind};

    #[test]
    fn topmost_terminal_at_picks_highest_stack_index_among_containing() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let c = world.spawn_empty().id();
        // A: left half (x 0..400), stack 5. B: right half (x 400..800), stack 3.
        // C: left half, stack 9 — overlaps A and sits on top.
        let node_a = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 5,
            ..ComputedNode::DEFAULT
        };
        let tf_a = UiGlobalTransform::from_xy(200.0, 300.0);
        let node_b = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 3,
            ..ComputedNode::DEFAULT
        };
        let tf_b = UiGlobalTransform::from_xy(600.0, 300.0);
        let node_c = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 9,
            ..ComputedNode::DEFAULT
        };
        let tf_c = UiGlobalTransform::from_xy(200.0, 300.0);
        let candidates = [
            (a, &node_a, &tf_a),
            (b, &node_b, &tf_b),
            (c, &node_c, &tf_c),
        ];

        assert_eq!(
            topmost_terminal_at(Vec2::new(600.0, 300.0), candidates.iter().copied()),
            Some(b),
            "a point only B contains must resolve to B"
        );
        assert_eq!(
            topmost_terminal_at(Vec2::new(100.0, 300.0), candidates.iter().copied()),
            Some(c),
            "where A and C overlap, the higher stack_index (C) wins"
        );
        assert_eq!(
            topmost_terminal_at(Vec2::new(2000.0, 2000.0), candidates.iter().copied()),
            None,
            "a point outside every node resolves to None"
        );
    }

    #[test]
    fn topmost_terminal_at_breaks_stack_index_ties_deterministically() {
        let mut world = World::new();
        let lower = world.spawn_empty().id();
        let higher = world.spawn_empty().id();
        // Two fully-overlapping nodes with the SAME stack_index (only reachable
        // before the first layout pass assigns indices). The winner must not
        // depend on candidate iteration order.
        let node = ComputedNode {
            size: Vec2::new(400.0, 600.0),
            stack_index: 0,
            ..ComputedNode::DEFAULT
        };
        let tf = UiGlobalTransform::from_xy(200.0, 300.0);
        let forward = [(lower, &node, &tf), (higher, &node, &tf)];
        let reversed = [(higher, &node, &tf), (lower, &node, &tf)];
        let winner = topmost_terminal_at(Vec2::new(100.0, 300.0), forward.iter().copied());
        assert_eq!(
            winner,
            topmost_terminal_at(Vec2::new(100.0, 300.0), reversed.iter().copied()),
            "tie resolution must not depend on iteration order"
        );
        assert_eq!(
            winner,
            Some(lower.max(higher)),
            "a stack_index tie resolves by Entity order, deterministically"
        );
    }

    #[test]
    fn mouse_disabled_terminal_drains_without_arming_a_gesture() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
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
    fn mouse_effects_on_entity_without_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .add_observer(on_terminal_mouse_effects);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TerminalMouseEffects {
            entity,
            effects: vec![MouseEffect::Scroll(3)],
        });
        app.update();
        // Reaching here proves the observer handles the missing-terminal path
        // without panicking; effect correctness is covered by the decide_* tests.
    }

    fn test_metrics() -> TerminalCellMetricsResource {
        use ozma_tty_renderer::CellMetrics;
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
    fn default_config_sets_button_cap_explicitly() {
        let cfg = OzmaMouseConfig::default();
        assert_eq!(
            cfg.buttons.max_protocol_events_per_frame, 8,
            "must NOT be ButtonConfig::default()'s 0"
        );
        assert_eq!(cfg.wheel.max_protocol_events_per_frame, 8);
        assert_eq!(cfg.cells_per_notch, 0.5);
        assert_eq!(
            cfg.double_click_timeout,
            std::time::Duration::from_millis(400)
        );
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
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(0),
                Vec2::new(10.0, 10.0),
                cfg
            ),
            1
        );
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(200),
                Vec2::new(11.0, 11.0),
                cfg
            ),
            2
        );
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(350),
                Vec2::new(12.0, 10.0),
                cfg
            ),
            3
        );
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(900),
                Vec2::new(12.0, 10.0),
                cfg
            ),
            1
        );
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
            origin: CellCoord { col: 1, row: 1 },
            side: Side::Left,
            ty: SelectionType::Simple,
            phase: DragPhase::Armed,
        };
        assert_eq!(armed.phase, DragPhase::Armed);
        assert_eq!((armed.origin.col, armed.origin.row), (1, 1));
        assert_eq!(armed.side, Side::Left);
        assert_eq!(armed.ty, SelectionType::Simple);

        let started = DragGesture {
            phase: DragPhase::Started,
            ..armed
        };
        assert_eq!(started.phase, DragPhase::Started);
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
    fn wheel_accumulator_resets_residual_on_target_change() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut acc = WheelAccumulator::default();
        acc.retarget(a);
        assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 0);
        acc.retarget(a);
        assert_eq!(
            accumulate_notches(&mut acc, 0.3, 0.5),
            1,
            "0.3 + 0.3 = 0.6 → one notch on the same target"
        );
        acc.retarget(b);
        assert_eq!(
            accumulate_notches(&mut acc, 0.3, 0.5),
            0,
            "switching target clears the carried residual"
        );
    }

    #[test]
    fn accumulator_zero_delta_does_not_reset_residual() {
        // A zero / negative-zero delta has no direction and must NOT trip the
        // sign-flip reset (signum(-0.0) == -1.0 would otherwise drop the carry).
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc, -0.0, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 1);
    }

    #[test]
    fn scrollback_up_returns_positive_viewport_scroll() {
        // Bevy +y (wheel up) → caller negates → engine notches negative → into history.
        let fx = decide_wheel(
            TermMode::empty(),
            -1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &WheelConfig::default(),
        );
        assert_eq!(fx, vec![MouseEffect::Scroll(3)]);
    }

    #[test]
    fn app_capture_wheel_forwards_bytes() {
        let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let fx = decide_wheel(
            modes,
            -1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &WheelConfig::default(),
        );
        assert!(matches!(fx.as_slice(), [MouseEffect::Write(b)] if !b.is_empty()));
    }
}
