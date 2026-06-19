//! Default mouse handler for the Ozma terminal: app reporting, local text
//! selection + copy, wheel scrollback, and Cmd-click hyperlink open. Reads Bevy
//! mouse input, hit-tests the cursor to a cell, and drives the engine's pure
//! `ButtonAction` / `WheelAction` routers, applying the result to the
//! `TerminalHandle` / `Clipboard`. Gated per entity by `InputDisabled`.

use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::CursorMoved;
use ozma_tty_engine::{
    ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, CellCoord, Column, Line,
    MouseButtonKind, Point, ProtocolModifiers, SelectionType, Side, TermMode, WheelAction,
    WheelConfig, WheelModifiers,
};
use std::time::Duration;

use crate::input::current_terminal_modifiers;

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
    #[cfg_attr(not(test), expect(dead_code, reason = "read by drag dispatch systems (Task 6)"))]
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
    #[cfg_attr(not(test), expect(dead_code, reason = "read by button dispatch systems (Task 6)"))]
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
    #[cfg_attr(not(test), expect(dead_code, reason = "called by button dispatch systems (Task 3)"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "called by cursor hit-test systems (Task 6)"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "called by button and wheel dispatch systems (Task 6)"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "called by dispatch systems (Task 6)"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "called by wheel dispatch system (Task 6)"))]
pub(crate) fn wheel_delta_cells(unit: MouseScrollUnit, y: f32, cell_h: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => y,
        MouseScrollUnit::Pixel => y / cell_h.max(1.0),
    }
}

/// Adds `delta_cells` to the accumulator and returns whole notches to emit
/// (positive = up/older), carrying the remainder. Resets on a sign flip and
/// caps the contributing delta to one notch to avoid a burst in the new direction.
#[cfg_attr(not(test), expect(dead_code, reason = "called by wheel dispatch system (Task 6)"))]
pub(crate) fn accumulate_notches(
    acc: &mut WheelAccumulator,
    delta_cells: f32,
    cells_per_notch: f32,
) -> i32 {
    let threshold = cells_per_notch.max(f32::EPSILON);
    let effective_delta = if acc.residual_cells != 0.0
        && acc.residual_cells.signum() != delta_cells.signum()
    {
        acc.residual_cells = 0.0;
        delta_cells.signum() * delta_cells.abs().min(threshold)
    } else {
        delta_cells
    };
    acc.residual_cells += effective_delta;
    let notches = (acc.residual_cells / threshold).trunc() as i32;
    if notches != 0 {
        acc.residual_cells -= notches as f32 * threshold;
    }
    notches
}

/// Pure wheel decision. `notches` is in the engine convention (negative =
/// up/older); callers negate the Bevy-derived up-positive value before calling.
#[cfg_attr(not(test), expect(dead_code, reason = "called by wheel dispatch system (Task 6)"))]
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
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_engine::{ButtonEvent, ButtonEventKind, MouseButtonKind};

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
        assert_eq!(accumulate_notches(&mut acc, -1.0, 0.5), -1);
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
