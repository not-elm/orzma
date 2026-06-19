//! Default mouse handler for the Ozma terminal: app reporting, local text
//! selection + copy, wheel scrollback, and Cmd-click hyperlink open. Reads Bevy
//! mouse input, hit-tests the cursor to a cell, and drives the engine's pure
//! `ButtonAction` / `WheelAction` routers, applying the result to the
//! `TerminalHandle` / `Clipboard`. Gated per entity by `InputDisabled`.

use bevy::input::mouse::{MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::CursorMoved;
use ozma_tty_engine::{
    ButtonConfig, CellCoord, Column, Line, MouseButtonKind, Point, ProtocolModifiers,
    SelectionType, Side, WheelConfig,
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
#[cfg_attr(not(test), expect(dead_code, reason = "constructed by decide_button (Task 3)"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragPhase {
    /// The button is held but the pointer has not left the origin cell.
    Armed,
    /// The pointer has crossed a cell boundary and selection is active.
    Started,
}

/// An in-progress button gesture: the held button, the selection anchor, and the
/// last cell a drag reached (dedup + lazy materialization).
#[cfg_attr(not(test), expect(dead_code, reason = "fields read by decide_button and drag dispatch systems (Task 3)"))]
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
    #[cfg_attr(not(test), expect(dead_code, reason = "read by button dispatch systems (Task 6)"))]
    pub(crate) click: ClickTracker,
    /// In-progress button gesture, or `None` when idle.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by button dispatch systems (Task 6)"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "called by selection dispatch systems (Task 6)"))]
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
}
