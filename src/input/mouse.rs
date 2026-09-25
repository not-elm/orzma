//! Shared mouse-dispatch plumbing for every `OrzmaTerminal` surface: both
//! `TerminalMouseDisabled` and `MouseClaimedByWebview` block a new press and
//! hover there, and only `TerminalMouseDisabled` also cancels a held gesture.

use crate::action::terminal::TerminalOpenUri;
use crate::input::InputPhase;
use crate::input::bindings::OrzmaMouseConfig;
use crate::input::focus::{MouseClaimedByWebview, TerminalMouseDisabled};
use crate::input::mouse::button::MouseButtonInputPlugin;
use crate::input::mouse::separator::SeparatorDragPlugin;
use crate::input::mouse::wheel::MouseWheelInputPlugin;
use crate::surface::OrzmaTerminal;
use bevy::input::mouse::{MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ComputedStackIndex, UiGlobalTransform};
use bevy::window::CursorMoved;
use bevy_orzma_tty_renderer::prelude::{TerminalCellMetricsResource, TerminalCells, TerminalView};
use bevy_orzmux::prelude::{CellSide, RequestTtyPointer};
use orzma_tty::prelude::{CellCoord, PointerInput, ProtocolModifiers, TerminalModifiers};

mod button;
mod gesture;
pub(in crate::input) mod separator;
mod webview;
mod wheel;

/// The order the frame's mouse messages are consumed in: a separator
/// grab decides first, so the dispatchers see the drag it started
/// before they route the same press.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::input::mouse) enum MousePhase {
    /// Separator grab, drag, and release.
    Grab,
    /// Terminal-selection and webview-pointer routing.
    Dispatch,
    /// Tearing down a finished separator drag.
    Retire,
}

/// Adds mouse button, wheel, and webview-pointer dispatch for every
/// terminal surface.
pub(super) struct MouseInputPlugin;

impl Plugin for MouseInputPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: never add `.ignore_deferred()` to this chain — it registers the
        // edge in `no_sync_edges`, dropping the `ApplyDeferred` that makes a
        // separator grab visible to the dispatchers on the press frame.
        app.add_plugins((
            SeparatorDragPlugin,
            MouseButtonInputPlugin,
            MouseWheelInputPlugin,
            webview::MouseWebviewPlugin,
        ))
        .init_resource::<OrzmaMouseConfig>()
        .configure_sets(
            Update,
            (MousePhase::Grab, MousePhase::Dispatch, MousePhase::Retire)
                .chain()
                .in_set(InputPhase::Dispatch),
        );
    }
}

/// True on any frame carrying a mouse button, cursor-move, or wheel
/// message. A cursor-only frame must still run so the dispatchers
/// retarget / reset.
fn on_any_mouse_message() -> impl SystemCondition<()> {
    on_message::<MouseButtonInput>
        .or_else(on_message::<CursorMoved>)
        .or_else(on_message::<MouseWheel>)
}

/// An operation to apply to the target terminal: a pointer event for its
/// backend to route, or a hyperlink to open.
#[derive(Debug, Clone, PartialEq)]
enum MouseEffect {
    Pointer(PointerInput),
    OpenUri(String),
}

/// Triggers the `EntityEvent` that applies `effect` to `entity`.
fn trigger_mouse_effect(commands: &mut Commands, entity: Entity, effect: MouseEffect) {
    match effect {
        MouseEffect::Pointer(input) => commands.trigger(RequestTtyPointer {
            terminal: entity,
            input,
        }),
        MouseEffect::OpenUri(uri) => commands.trigger(TerminalOpenUri { entity, uri }),
    }
}

/// Resolves the window-space physical cursor to a cell on the terminal node.
///
/// Any position is projected and the resulting column/row is clamped to
/// `1..=cols × 1..=rows`, so a cursor outside the node resolves to the nearest
/// edge cell. Returns `None` only when the node has no projectable geometry
/// (zero size or a non-invertible transform).
fn cell_at_cursor(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
) -> Option<(CellCoord, CellSide)> {
    let local = node
        .normalize_point(*transform, cursor_phys)
        .map(|n| (n + Vec2::splat(0.5)) * node.size)?;
    Some(cell_at_local(local, cell_w, cell_h, cols, rows))
}

/// 1-indexed `(CellCoord, CellSide)` of the cell at pane-local physical
/// `local`, clamped to `1..=cols` × `1..=rows`. `CellSide` is `Left` in the
/// left half, and a point past the last column reads as the right half of
/// the last cell.
fn cell_at_local(
    local: Vec2,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
) -> (CellCoord, CellSide) {
    let col_f = (local.x / cell_w).max(0.0);
    let row_f = (local.y / cell_h).max(0.0);
    let col = (col_f.floor() as u32 + 1).min(cols as u32).max(1);
    let row = (row_f.floor() as u32 + 1).min(rows as u32).max(1);
    let side = if col_f < f32::from(cols) && col_f - col_f.floor() < 0.5 {
        CellSide::Left
    } else {
        CellSide::Right
    };
    (CellCoord { col, row }, side)
}

/// A terminal-surface query matching every mouse-enabled `OrzmaTerminal`
/// surface.
type TerminalSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ComputedNode,
        &'static ComputedStackIndex,
        &'static UiGlobalTransform,
        &'static TerminalView,
        &'static TerminalCells,
    ),
    (
        With<OrzmaTerminal>,
        Without<TerminalMouseDisabled>,
        Without<MouseClaimedByWebview>,
    ),
>;

/// The surface query for the terminal a held button is locked to: every
/// `OrzmaTerminal` without `TerminalMouseDisabled`. Unlike
/// [`TerminalSurfaces`], a webview claim does not drop a terminal from it,
/// so a drag that crosses an inline webview keeps its terminal.
type HeldSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        &'static ComputedNode,
        &'static UiGlobalTransform,
        &'static TerminalView,
        &'static TerminalCells,
    ),
    (With<OrzmaTerminal>, Without<TerminalMouseDisabled>),
>;

/// The `(entity, node, stack, transform)` candidates `topmost_surface_at`
/// hit-tests, projected from the surface query.
fn hit_candidates<'a>(
    terminals: &'a TerminalSurfaces<'_, '_>,
) -> impl Iterator<
    Item = (
        Entity,
        &'a ComputedNode,
        &'a ComputedStackIndex,
        &'a UiGlobalTransform,
    ),
> {
    terminals
        .iter()
        .map(|(e, node, stack, transform, _, _)| (e, node, stack, transform))
}

/// The `(cell_w, cell_h)` pitch in physical px, floored and clamped to
/// `>= 1` so a degenerate metric cannot divide by zero.
fn cell_dims(metrics: &TerminalCellMetricsResource) -> (f32, f32) {
    (
        metrics.metrics.advance_phys.floor().max(1.0),
        metrics.metrics.line_height_phys.floor().max(1.0),
    )
}

/// The mouse-report modifier bits for the held keys; a held Super (Cmd on
/// macOS) sets no bit.
fn protocol_mods(held: &TerminalModifiers) -> ProtocolModifiers {
    ProtocolModifiers {
        shift: held.shift,
        ctrl: held.ctrl,
        alt: held.alt,
        meta: false,
    }
}

/// Read-only hit-test context for one gather run: the terminal node
/// geometry, cell pitch, the view's dimensions, and the cells a
/// hyperlink lookup resolves against.
struct CellContext<'a> {
    node: &'a ComputedNode,
    transform: &'a UiGlobalTransform,
    view: &'a TerminalView,
    cells: &'a TerminalCells,
    cell_w: f32,
    cell_h: f32,
}

impl<'a> CellContext<'a> {
    /// The context of the terminal a held button is locked to, or `None`
    /// when it is gone or mouse-disabled.
    fn held(
        held_surfaces: &'a HeldSurfaces<'_, '_>,
        target: Entity,
        cell_w: f32,
        cell_h: f32,
    ) -> Option<Self> {
        let (node, transform, view, cells) = held_surfaces.get(target).ok()?;
        Some(Self {
            node,
            transform,
            view,
            cells,
            cell_w,
            cell_h,
        })
    }

    fn hit(&self, cursor_phys: Vec2) -> Option<(CellCoord, CellSide)> {
        cell_at_cursor(
            self.node,
            self.transform,
            cursor_phys,
            self.cell_w,
            self.cell_h,
            self.view.cols,
            self.view.rows,
        )
    }

    /// The URI of the OSC 8 hyperlink on the 1-based `cell`, if any.
    fn link_at(&self, cell: CellCoord) -> Option<String> {
        self.cells
            .hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16)
            .map(|(_id, uri)| uri.as_str().to_string())
    }
}

/// Resolves `target` to its `CellContext` at the given cell pitch, or
/// `None` when it is no longer a live surface.
fn cell_context_for<'a>(
    terminals: &'a TerminalSurfaces<'_, '_>,
    target: Entity,
    cell_w: f32,
    cell_h: f32,
) -> Option<CellContext<'a>> {
    let (_, node, _, transform, view, cells) = terminals.get(target).ok()?;
    Some(CellContext {
        node,
        transform,
        view,
        cells,
        cell_w,
        cell_h,
    })
}

#[cfg(test)]
mod test_support {
    use super::*;
    use bevy::window::PrimaryWindow;

    #[derive(Resource, Default)]
    pub(super) struct CapturedEffects(pub(super) Vec<MouseEffect>);

    pub(super) fn add_effect_capture_observers(app: &mut App) {
        app.add_observer(
            |ev: On<RequestTtyPointer>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::Pointer(ev.input));
            },
        )
        .add_observer(
            |ev: On<TerminalOpenUri>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::OpenUri(ev.uri.clone()));
            },
        );
    }

    pub(super) fn set_phys_cursor(app: &mut App, phys: Vec2) {
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

    pub(super) fn test_metrics() -> TerminalCellMetricsResource {
        use bevy_orzma_tty_renderer::prelude::CellMetrics;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keyboard::current_terminal_modifiers;

    /// Asserts that `cell_at_local` yields 1-indexed cell coordinates,
    /// clamps them to the grid bounds, and reports which half of the cell
    /// was hit, a point past the last column reading as its right half.
    ///
    /// Case: the user clicks the pane's top-left corner, drags far past
    /// the bottom-right cell, and clicks the right half of a cell in the
    /// top row.
    #[test]
    fn cell_at_local_is_one_indexed_and_clamped() {
        let (cell, side) = cell_at_local(Vec2::new(0.0, 0.0), 10.0, 20.0, 80, 24);
        assert_eq!((cell.col, cell.row), (1, 1));
        assert_eq!(side, CellSide::Left);
        let (cell, side) = cell_at_local(Vec2::new(10_002.0, 10_000.0), 10.0, 20.0, 80, 24);
        assert_eq!((cell.col, cell.row), (80, 24));
        assert_eq!(side, CellSide::Right);
        let (cell, side) = cell_at_local(Vec2::new(17.0, 5.0), 10.0, 20.0, 80, 24);
        assert_eq!(cell.col, 2);
        assert_eq!(side, CellSide::Right);
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

    /// Asserts that `protocol_mods` reports Ctrl and Shift from the key
    /// state and leaves Alt and Meta clear.
    ///
    /// Case: the user holds Ctrl+Shift while clicking.
    #[test]
    fn protocol_mods_sets_ctrl_and_shift() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::ShiftLeft);
        let mods = protocol_mods(&current_terminal_modifiers(&keys));
        assert!(mods.ctrl);
        assert!(mods.shift);
        assert!(!mods.alt);
        assert!(!mods.meta);
    }

    /// Asserts that `protocol_mods` sets no modifier bit for a held Super.
    ///
    /// Case: on macOS the user holds Cmd, the hyperlink modifier, while
    /// spinning the wheel over nvim.
    #[test]
    fn protocol_mods_sets_no_bit_for_a_held_super() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::SuperLeft);
        assert_eq!(
            protocol_mods(&current_terminal_modifiers(&keys)),
            ProtocolModifiers::default()
        );
    }
}
