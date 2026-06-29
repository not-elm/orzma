//! Shared mouse-dispatch plumbing for every `OzmaTerminal` surface. Hosts the
//! `MouseInputPlugin` aggregator and the hit-test / effect primitives
//! (`CellContext`, `cell_at_*`, the `MouseEffect` IR, and `trigger_mouse_effects`)
//! shared by the per-path dispatchers in `button` and `wheel`. Gated per entity
//! by `MouseDisabled`, so dispatch runs in both `AppMode`s for surfaces that still
//! own the mouse.

use crate::input::bindings::OzmaMouseConfig;
use crate::input::mouse::button::MouseButtonInputPlugin;
use crate::input::mouse::wheel::MouseWheelInputPlugin;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use ozma_terminal::{
    TerminalMouseWrite, TerminalOpenUri, TerminalSelectionClear, TerminalSelectionCopy,
    TerminalSelectionStart, TerminalSelectionUpdate, TerminalViewportScroll,
};
use ozma_tty_engine::{CellCoord, Point, SelectionType, Side};
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

/// Host-private decision IR: the deciders (`decide_button` / `decide_wheel`)
/// return an ordered `Vec` of these, which `trigger_mouse_effects` fans out
/// to per-operation `EntityEvent`s on the target terminal.
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
    Scroll(i32),
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
            MouseEffect::Scroll(lines) => {
                commands.trigger(TerminalViewportScroll { entity, lines })
            }
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

#[cfg(test)]
mod tests {
    use super::button::*;
    use super::wheel::*;
    use super::*;
    use crate::input::focus::MouseDisabled;
    use crate::input::gesture::{DragGesture, DragPhase, OzmaMouseGesture, WheelAccumulator};
    use crate::webview_pointer::topmost_surface_at;
    use bevy::input::ButtonState;
    use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel};
    use bevy::window::{CursorMoved, PrimaryWindow};
    use ozma_terminal::{Clipboard, OzmaTerminal};
    use ozma_tty_engine::{
        ButtonConfig, ButtonEvent, ButtonEventKind, MouseButtonKind, ProtocolModifiers,
        SelectionType, TermMode, TerminalHandle, WheelAction, WheelConfig, WheelModifiers,
    };
    use ozma_tty_renderer::TerminalCellMetricsResource;

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

    #[derive(Resource, Default)]
    struct CapturedEffects(Vec<MouseEffect>);

    fn add_effect_capture_observers(app: &mut App) {
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
            |ev: On<TerminalViewportScroll>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::Scroll(ev.lines));
            },
        )
        .add_observer(
            |ev: On<TerminalOpenUri>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::OpenUri(ev.uri.clone()));
            },
        );
    }

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

    fn set_phys_cursor(app: &mut App, phys: Vec2) {
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

    #[test]
    fn effects_from_wheel_action_maps_each_variant() {
        assert_eq!(effects_from_wheel_action(WheelAction::Noop), vec![]);
        assert_eq!(
            effects_from_wheel_action(WheelAction::WriteToPty(b"x".to_vec())),
            vec![MouseEffect::Write(b"x".to_vec())]
        );
        assert_eq!(
            effects_from_wheel_action(WheelAction::ScrollViewport(3)),
            vec![MouseEffect::Scroll(3)]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn horizontal_modifiers_strip_shift_on_macos() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ShiftLeft);
        let cfg = OzmaMouseConfig::default();
        let mods = build_wheel_modifiers_horizontal(&keys, &cfg);
        assert!(
            !mods.shift,
            "macOS converts Shift+wheel to horizontal at the OS level; the report must not carry the Shift bit"
        );
    }

    fn make_wheel_app(enable_modes: &[u8]) -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseWheel>()
            .init_resource::<OzmaMouseConfig>()
            .init_resource::<WheelAccumulator>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Clipboard>()
            .init_resource::<CapturedEffects>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_wheel);
        add_effect_capture_observers(&mut app);

        let mut handle = TerminalHandle::detached(100, 37);
        handle.advance(enable_modes);
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

    fn write_wheel(app: &mut App, x: f32, y: f32) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x,
                y,
                window: Entity::PLACEHOLDER,
            });
    }

    /// Sign of `MouseWheel.x` for a physical-right trackpad scroll on this
    /// platform: negative on macOS (winit's PixelDelta is opposite X11/Wayland),
    /// positive elsewhere. Lets the direction tests assert the same SGR button
    /// on every target instead of being cfg-gated to macOS.
    fn phys_right_sign() -> f32 {
        if cfg!(target_os = "macos") { -1.0 } else { 1.0 }
    }

    fn disable_axis_lock(app: &mut App) {
        app.insert_resource(OzmaMouseConfig {
            axis_lock_ratio: 0.0,
            ..default()
        });
    }

    #[test]
    fn dispatch_pure_horizontal_right_emits_sgr_67() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5 * phys_right_sign(), 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
            "a physical-right wheel in mouse mode must emit an SGR wheel-right (cb 67) report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_horizontal_left_emits_sgr_66() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, -0.5 * phys_right_sign(), 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<66;"))),
            "a physical-left wheel in mouse mode must emit an SGR wheel-left (cb 66) report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_diagonal_emits_both_axes() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        // Disable the dominant-axis lock so a diagonal keeps both axes; this
        // test guards the batching (both axes in ONE trigger), not the lock.
        disable_axis_lock(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5 * phys_right_sign(), -0.5);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<65;"))),
            "vertical (down, cb 65) report missing: {:?}",
            cap.0
        );
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
            "horizontal (right, cb 67) report missing: {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_axis_lock_drops_jitter_during_vertical_scroll() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        // Vertical-dominant swipe whose horizontal component (0.6 cells) is on
        // its own past cells_per_notch (0.5) and WOULD emit a notch unlocked;
        // |x|/hypot = 0.29 < 0.9, so the default lock must drop it (no cb 66/67).
        // A smaller jitter would not discriminate — it makes no notch either way.
        write_wheel(&mut app, 0.6, -2.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        let has = |needle: &[u8]| {
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(needle)))
        };
        assert!(has(b"\x1b[<65;"), "vertical (down, cb 65) report missing");
        assert!(
            !has(b"\x1b[<66;") && !has(b"\x1b[<67;"),
            "off-axis jitter must NOT emit a horizontal report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_horizontal_without_mouse_mode_emits_no_report() {
        let mut app = make_wheel_app(b"");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().all(|e| !matches!(e, MouseEffect::Write(_))),
            "horizontal wheel outside a mouse mode must not emit a report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn pixel_horizontal_sensitivity_matches_vertical() {
        // Equal Pixel-unit deltas on both axes must emit equal report counts.
        // Regression: horizontal divided ev.x by cell_w (~half of cell_h), so a
        // given finger distance fired ~2x the notches and scrolled too far.
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        // Disable the dominant-axis lock; this test compares per-axis
        // sensitivity, which needs both axes to survive an equal-delta gesture.
        disable_axis_lock(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Pixel,
                x: 16.0 * phys_right_sign(),
                y: 16.0,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        let count = |needle: &[u8]| -> usize {
            cap.0
                .iter()
                .filter_map(|e| match e {
                    MouseEffect::Write(b) => Some(b),
                    _ => None,
                })
                .map(|b| b.windows(needle.len()).filter(|w| *w == needle).count())
                .sum()
        };
        // test_metrics: cell_w = 8, cell_h = 16. y=16 → up reports (cb 64);
        // a physical-right x → right reports (cb 67).
        let vertical = count(b"\x1b[<64;");
        let horizontal = count(b"\x1b[<67;");
        assert!(
            vertical > 0 && horizontal > 0,
            "both axes must emit reports, got v={vertical} h={horizontal}"
        );
        assert_eq!(
            horizontal, vertical,
            "equal Pixel deltas must emit equal report counts (horizontal sensitivity \
             must match vertical), got v={vertical} h={horizontal}"
        );
    }
}
