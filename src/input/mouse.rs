//! Shared mouse-dispatch plumbing for every `OzmaTerminal` surface. Hosts the
//! `MouseInputPlugin` aggregator, the hit-test primitives (`CellContext`,
//! `cell_at_*`, `hit_candidates`, the `TerminalSurfaces` query alias) shared by
//! the per-path dispatchers in `button` and `wheel`, and the `MouseEffect` IR +
//! `trigger_mouse_effects` used by the `button` path only (the `wheel` path
//! triggers its `EntityEvent`s directly). Gated per entity by `MouseDisabled`,
//! so dispatch runs in both `AppMode`s for surfaces that still own the mouse.

use crate::action::terminal::{
    TerminalMouseWrite, TerminalOpenUri, TerminalSelectionClear, TerminalSelectionCopy,
    TerminalSelectionStart, TerminalSelectionUpdate,
};
use crate::input::bindings::OzmaMouseConfig;
use crate::input::focus::MouseDisabled;
use crate::input::mouse::button::MouseButtonInputPlugin;
use crate::input::mouse::wheel::MouseWheelInputPlugin;
use bevy::input::mouse::{MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::CursorMoved;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{CellCoord, Point, SelectionType, Side, TermMode, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::TerminalGrid;

mod button;
mod wheel;

/// Aggregates the per-path mouse dispatch plugins and the shared config
/// resource. The button and wheel dispatchers live in `button` / `wheel` and
/// register themselves.
pub(super) struct MouseInputPlugin;

impl Plugin for MouseInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((MouseButtonInputPlugin, MouseWheelInputPlugin))
            .init_resource::<OzmaMouseConfig>();
    }
}

/// Run condition shared by the button and wheel dispatchers: true on any frame
/// carrying a mouse message (button, cursor move, or wheel). A cursor-only frame
/// must still run so the dispatchers retarget / reset; defining it once keeps the
/// two per-file plugins' gating in lockstep.
pub(super) fn on_any_mouse_message() -> impl SystemCondition<()> {
    on_message::<MouseButtonInput>
        .or(on_message::<CursorMoved>)
        .or(on_message::<MouseWheel>)
}

/// Host-private decision IR for the button path: `decide_button` returns an
/// ordered `Vec` of these, which `trigger_mouse_effects` fans out to
/// per-operation `EntityEvent`s on the target terminal. (The wheel path triggers
/// its `EntityEvent`s directly from `WheelAction`, bypassing this IR.)
#[derive(Debug, Clone, PartialEq)]
enum MouseEffect {
    Write(Vec<u8>),
    SelStart {
        point: Point,
        side: Side,
        ty: SelectionType,
    },
    SelUpdate {
        point: Point,
        side: Side,
    },
    SelClear,
    Copy,
    OpenUri(String),
}

/// Fans an ordered `Vec<MouseEffect>` out to per-operation `EntityEvent`s on
/// `entity`, preserving order (Bevy's command queue is FIFO and each trigger
/// resolves before the next).
fn trigger_mouse_effects(commands: &mut Commands, entity: Entity, effects: Vec<MouseEffect>) {
    for effect in effects {
        match effect {
            MouseEffect::Write(bytes) => commands.trigger(TerminalMouseWrite { entity, bytes }),
            MouseEffect::SelStart { point, side, ty } => {
                commands.trigger(TerminalSelectionStart {
                    entity,
                    point,
                    side,
                    ty,
                });
            }
            MouseEffect::SelUpdate { point, side } => {
                commands.trigger(TerminalSelectionUpdate {
                    entity,
                    point,
                    side,
                });
            }
            MouseEffect::SelClear => commands.trigger(TerminalSelectionClear { entity }),
            MouseEffect::Copy => commands.trigger(TerminalSelectionCopy { entity }),
            MouseEffect::OpenUri(uri) => commands.trigger(TerminalOpenUri { entity, uri }),
        }
    }
}

/// 1-indexed `(CellCoord, Side)` of the cell at pane-local physical `local`,
/// clamped to `1..=cols` × `1..=rows`. `Side` is `Left` in the left half.
fn cell_at_local(local: Vec2, cell_w: f32, cell_h: f32, cols: u16, rows: u16) -> (CellCoord, Side) {
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
) -> Option<(CellCoord, Side)> {
    let local = node
        .normalize_point(*transform, cursor_phys)
        .map(|n| (n + Vec2::splat(0.5)) * node.size)?;
    Some(cell_at_local(local, cell_w, cell_h, cols, rows))
}

/// The terminal-surface query shared by the button and wheel dispatchers,
/// aliased so the long type is not repeated across their helper signatures.
type TerminalSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static TerminalHandle,
        &'static ComputedNode,
        &'static UiGlobalTransform,
        &'static TerminalGrid,
    ),
    (With<OzmaTerminal>, Without<MouseDisabled>),
>;

/// The `(entity, node, transform)` candidates `topmost_surface_at` hit-tests,
/// projected from the surface query — one adapter shared by both dispatchers.
fn hit_candidates<'a>(
    terminals: &'a TerminalSurfaces<'_, '_>,
) -> impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)> {
    terminals
        .iter()
        .map(|(e, _, node, transform, _)| (e, node, transform))
}

/// The `(cell_w, cell_h)` pitch in physical px, floored and clamped to `>= 1` so
/// a degenerate metric cannot divide by zero. Shared by both dispatchers' cursor
/// → cell projection.
fn cell_pitch(metrics: &TerminalCellMetricsResource) -> (f32, f32) {
    (
        metrics.metrics.advance_phys.floor().max(1.0),
        metrics.metrics.line_height_phys.floor().max(1.0),
    )
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

/// Resolves `target` to its `(CellContext, TermMode)` at the given cell pitch,
/// or `None` when it is no longer a live surface. Shared by the button and wheel
/// dispatchers so both build a hit-test context the same way.
fn cell_context_for<'a>(
    terminals: &'a TerminalSurfaces<'_, '_>,
    target: Entity,
    cell_w: f32,
    cell_h: f32,
) -> Option<(CellContext<'a>, TermMode)> {
    let (_, handle, node, transform, grid) = terminals.get(target).ok()?;
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };
    Some((ctx, handle.current_modes()))
}

#[cfg(test)]
mod test_support {
    use super::*;
    use bevy::window::PrimaryWindow;
    use ozma_tty_renderer::TerminalCellMetricsResource;

    #[derive(Resource, Default)]
    pub(super) struct CapturedEffects(pub(super) Vec<MouseEffect>);

    pub(super) fn add_effect_capture_observers(app: &mut App) {
        app.add_observer(
            |ev: On<TerminalMouseWrite>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::Write(ev.bytes.clone()));
            },
        )
        .add_observer(
            |ev: On<TerminalSelectionStart>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::SelStart {
                    point: ev.point,
                    side: ev.side,
                    ty: ev.ty,
                });
            },
        )
        .add_observer(
            |ev: On<TerminalSelectionUpdate>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::SelUpdate {
                    point: ev.point,
                    side: ev.side,
                });
            },
        )
        .add_observer(
            |_ev: On<TerminalSelectionClear>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::SelClear);
            },
        )
        .add_observer(
            |_ev: On<TerminalSelectionCopy>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::Copy);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webview_pointer::topmost_surface_at;

    #[test]
    fn topmost_surface_at_picks_highest_stack_index_among_containing() {
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
            topmost_surface_at(Vec2::new(600.0, 300.0), candidates.iter().copied()),
            Some(b),
            "a point only B contains must resolve to B"
        );
        assert_eq!(
            topmost_surface_at(Vec2::new(100.0, 300.0), candidates.iter().copied()),
            Some(c),
            "where A and C overlap, the higher stack_index (C) wins"
        );
        assert_eq!(
            topmost_surface_at(Vec2::new(2000.0, 2000.0), candidates.iter().copied()),
            None,
            "a point outside every node resolves to None"
        );
    }

    #[test]
    fn topmost_surface_at_breaks_stack_index_ties_deterministically() {
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
        let winner = topmost_surface_at(Vec2::new(100.0, 300.0), forward.iter().copied());
        assert_eq!(
            winner,
            topmost_surface_at(Vec2::new(100.0, 300.0), reversed.iter().copied()),
            "tie resolution must not depend on iteration order"
        );
        assert_eq!(
            winner,
            Some(lower.max(higher)),
            "a stack_index tie resolves by Entity order, deterministically"
        );
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
}
