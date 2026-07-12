//! Visible pane-divider handle bars, the resize hover cursor, and mouse-drag
//! pane resize, built on `crate::multiplexer::layout`'s pure geometry
//! (`MultiplexerLayout::divider_rects` / `resize_split_at`, `divider_at`,
//! `DividerRect` / `ChildSide` — Task 14a).
//!
//! Three systems, self-contained in this one file (a `ui` module may freely
//! read mouse messages): `reconcile_divider_handles` spawns/despawns one
//! handle bar per divider of the active window, parented to its
//! `WindowContainer` and positioned from the same physical-px geometry
//! `crate::multiplexer::pane::layout::apply_layout` uses for panes;
//! `divider_hover_feedback` brightens the hovered handle and sets a
//! `ColResize` / `RowResize` cursor, running after `InputPhase::Hover` so it
//! overrides the hyperlink baseline cursor; `divider_drag` turns a Left
//! press-drag-release cycle over a divider into `resize_split_at` calls.

use crate::input::InputPhase;
use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::layout::{ChildSide, PaneRect, SplitAxis, divider_at};
use crate::multiplexer::pane::layout::PANE_GAP_PX;
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerLayoutComp};
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::ui::ComputedNode;
use bevy::window::{CursorIcon, CursorMoved, PrimaryWindow, SystemCursorIcon, Window};

/// Subtle grey background for an un-hovered divider handle. The theme module
/// is gone (see `ui/multiplexer/confirm_prompt.rs`); this file declares its
/// own colors the same way.
const DIVIDER_COLOR: Color = Color::srgb(0.35, 0.35, 0.35);

/// Accent background for the hovered divider handle, brightened over
/// `DIVIDER_COLOR` so the resize grab zone reads as interactive.
const DIVIDER_HOVER_COLOR: Color = Color::srgb(0.55, 0.75, 1.0);

/// Divider hit-test tolerance, in PHYSICAL px — the same space as
/// `MultiplexerLayout::divider_rects`. Widens the 1px gap's grab zone along
/// its major axis so the resize cursor and drag start are easy to hit.
const DIVIDER_GRAB_TOL_PX: f32 = 4.0;

/// Stacking order for divider handles: above ordinary pane content (which has
/// no explicit `GlobalZIndex`, i.e. 0) but below transient overlays (the IME
/// overlay's 200, the confirm prompt's 330 — see `ui/ime_overlay.rs` /
/// `ui/multiplexer/confirm_prompt.rs`).
const DIVIDER_HANDLE_Z: i32 = 50;

/// Minimum split ratio either side of a drag is clamped to. Mirrors
/// `crate::multiplexer::pane`'s `MIN_PANE_FRAC` convention (that constant is
/// private to its module, so this is a local re-declaration of the same 0.05
/// value, not an import).
const MIN_PANE_FRAC: f32 = 0.05;

/// Registers the divider-handle visuals, the resize hover cursor, and
/// mouse-drag pane resize.
pub(super) struct DividerHandlePlugin;

impl Plugin for DividerHandlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DividerDrag>()
            // `MouseButtonInput` / `CursorMoved` are normally registered by
            // bevy's `InputPlugin` / `WindowPlugin`; `add_message` here too
            // (idempotent) so `divider_drag`'s `MessageReader`s and its
            // `on_message` run condition have a `Messages<T>` resource to
            // read even when this plugin is exercised without those plugins,
            // mirroring `ResizePanePlugin` / `ZoomPanePlugin`.
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .add_systems(
                Update,
                (
                    reconcile_divider_handles.run_if(
                        window_container_computed_changed.or_else(active_layout_tree_changed),
                    ),
                    divider_hover_feedback.after(InputPhase::Hover),
                    divider_drag
                        .run_if(on_message::<MouseButtonInput>.or_else(on_message::<CursorMoved>)),
                ),
            );
    }
}

/// A visible bar filling one divider's reserved gap. `window` is the owning
/// window entity and `index` its position in that window's `divider_rects`
/// output — both stable within a frame, letting a layout change despawn this
/// window's handles before respawning the current set, and letting the hover
/// system correlate a `divider_at` hit back to its bar.
#[derive(Component)]
struct DividerHandle {
    window: Entity,
    index: usize,
}

/// One in-progress divider drag: the target split's path/axis/extent
/// (captured at press time) plus the last physical-px cursor position along
/// the split's axis, used to compute each `CursorMoved`'s incremental delta.
struct ActiveDrag {
    path: Vec<ChildSide>,
    axis: SplitAxis,
    axis_extent: f32,
    last_cursor: Vec2,
}

/// The in-progress divider drag, if any; `None` outside a Left
/// press-drag-release cycle.
#[derive(Resource, Default)]
struct DividerDrag(Option<ActiveDrag>);

/// Whether the active window's `WindowContainer` was resized this frame.
/// Mirrors `crate::multiplexer::pane::layout`'s run condition of the same
/// name (private to that module, so re-declared here).
fn window_container_computed_changed(
    containers: Query<(), (With<WindowContainer>, Changed<ComputedNode>)>,
) -> bool {
    !containers.is_empty()
}

/// Whether the active window's layout tree changed this frame. Mirrors
/// `crate::multiplexer::pane::layout`'s run condition of the same name.
fn active_layout_tree_changed(
    windows: Query<
        (),
        (
            With<ActiveMultiplexerWindow>,
            Changed<MultiplexerLayoutComp>,
        ),
    >,
) -> bool {
    !windows.is_empty()
}

/// Despawns and respawns the active window's handle bars from its current
/// `divider_rects`, gated to frames where the active layout tree or the
/// container's computed size changed (the two run conditions above). Each
/// handle is positioned with the same physical-px-times-`inverse_scale_factor`
/// conversion `apply_layout` uses for pane rects, and carries a `GlobalZIndex`
/// so hover-brighten renders over pane content.
fn reconcile_divider_handles(
    mut commands: Commands,
    active_windows: Query<(Entity, &MultiplexerLayoutComp), With<ActiveMultiplexerWindow>>,
    containers: Query<(Entity, &WindowContainer, &ComputedNode)>,
    handles: Query<(Entity, &DividerHandle)>,
) {
    let Ok((window, layout)) = active_windows.single() else {
        return;
    };
    let Some((container, _, computed)) = containers.iter().find(|(_, c, _)| c.window == window)
    else {
        return;
    };
    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    let dividers = layout.0.divider_rects(area, PANE_GAP_PX);

    for (entity, handle) in handles.iter() {
        if handle.window == window {
            commands.entity(entity).despawn();
        }
    }
    for (index, divider) in dividers.iter().enumerate() {
        let (left, top, width, height) =
            handle_node_rect(divider.rect, computed.inverse_scale_factor);
        commands.spawn((
            DividerHandle { window, index },
            Node {
                position_type: PositionType::Absolute,
                left,
                top,
                width,
                height,
                ..default()
            },
            BackgroundColor(DIVIDER_COLOR),
            GlobalZIndex(DIVIDER_HANDLE_Z),
            ChildOf(container),
        ));
    }
}

/// Brightens the hovered divider's handle to the accent color and sets a
/// `ColResize` / `RowResize` cursor on the primary window; leaves the cursor
/// untouched when no divider is hovered so `hyperlink_hover_and_cursor`'s
/// baseline reasserts. Runs `.after(InputPhase::Hover)` so it overrides that
/// baseline while the pointer is over a divider.
///
/// **Px-space:** `divider_rects` is PHYSICAL px; the window's cursor position
/// is LOGICAL px. Converts by dividing by the container's
/// `inverse_scale_factor` (equivalently, multiplying by the scale factor) —
/// the same conversion `divider_drag` uses.
fn divider_hover_feedback(
    mut handles: Query<(&DividerHandle, &mut BackgroundColor)>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    active_windows: Query<(Entity, &MultiplexerLayoutComp), With<ActiveMultiplexerWindow>>,
    containers: Query<(&WindowContainer, &ComputedNode)>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((window, layout)) = active_windows.single() else {
        return;
    };
    let Some((_, computed)) = containers.iter().find(|(c, _)| c.window == window) else {
        return;
    };
    let Ok(win) = primary_window.single() else {
        return;
    };
    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    let dividers = layout.0.divider_rects(area, PANE_GAP_PX);
    let hovered = win.cursor_position().and_then(|cursor_logical| {
        let cursor_physical = cursor_logical / computed.inverse_scale_factor;
        divider_at(&dividers, cursor_physical, DIVIDER_GRAB_TOL_PX)
    });

    for (handle, mut bg) in handles.iter_mut() {
        if handle.window != window {
            continue;
        }
        let want = if Some(handle.index) == hovered {
            DIVIDER_HOVER_COLOR
        } else {
            DIVIDER_COLOR
        };
        if bg.0 != want {
            bg.0 = want;
        }
    }

    let Some(index) = hovered else {
        return;
    };
    let icon = match dividers[index].axis {
        SplitAxis::Vertical => SystemCursorIcon::ColResize,
        SplitAxis::Horizontal => SystemCursorIcon::RowResize,
    };
    if let Ok(mut cursor) = cursor_icons.single_mut()
        && !matches!(&*cursor, CursorIcon::System(existing) if *existing == icon)
    {
        *cursor = CursorIcon::System(icon);
    }
}

/// Press → drag → resize on a divider: a Left `MouseButtonInput` press whose
/// (physical-px) cursor hits a divider's grab zone starts a drag, recording
/// that divider's path/axis/extent; each `CursorMoved` while dragging applies
/// `resize_split_at`'s `delta_frac` along the split's axis; a Left release
/// clears the drag. Self-contained: does not touch
/// `crate::input::mouse::button`, whose surface hit-test always misses the
/// 1px inter-pane gap, so the two dispatchers never contend.
///
/// **Px-space:** same physical-px conversion as `divider_hover_feedback`.
/// **Sign:** positive `delta_frac` grows the split's first child
/// (`resize_split_at`'s convention); dragging a vertical divider right
/// (`cursor.x` increasing) yields a positive `delta_px`, growing the left
/// (first) child, as expected.
///
/// **Change detection:** only writes `layout.0` (through `DerefMut`, which
/// fires `Changed`) when `resize_split_at` reports a real change — the
/// clone-then-assign-on-true pattern `resize_pane` / `zoom_pane` use — so a
/// zero-delta `CursorMoved` (no motion along the split's axis) never
/// spuriously trips `Changed<MultiplexerLayoutComp>`.
fn divider_drag(
    mut drag: ResMut<DividerDrag>,
    mut windows: Query<(Entity, &mut MultiplexerLayoutComp), With<ActiveMultiplexerWindow>>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut cursor_moved: MessageReader<CursorMoved>,
    containers: Query<(&WindowContainer, &ComputedNode)>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((window, mut layout)) = windows.single_mut() else {
        buttons.clear();
        cursor_moved.clear();
        return;
    };
    let Some((_, computed)) = containers.iter().find(|(c, _)| c.window == window) else {
        buttons.clear();
        cursor_moved.clear();
        return;
    };
    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    let inverse_scale = computed.inverse_scale_factor;

    for ev in buttons.read() {
        if ev.button != MouseButton::Left {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => {
                let Ok(win) = primary_window.single() else {
                    continue;
                };
                let Some(cursor_logical) = win.cursor_position() else {
                    continue;
                };
                let cursor_physical = cursor_logical / inverse_scale;
                let dividers = layout.0.divider_rects(area, PANE_GAP_PX);
                if let Some(index) = divider_at(&dividers, cursor_physical, DIVIDER_GRAB_TOL_PX) {
                    let hit = &dividers[index];
                    drag.0 = Some(ActiveDrag {
                        path: hit.path.clone(),
                        axis: hit.axis,
                        axis_extent: hit.axis_extent,
                        last_cursor: cursor_physical,
                    });
                }
            }
            ButtonState::Released => drag.0 = None,
        }
    }

    let Some(active) = drag.0.as_mut() else {
        cursor_moved.clear();
        return;
    };
    for ev in cursor_moved.read() {
        let cursor_physical = ev.position / inverse_scale;
        let delta_px = match active.axis {
            SplitAxis::Vertical => cursor_physical.x - active.last_cursor.x,
            SplitAxis::Horizontal => cursor_physical.y - active.last_cursor.y,
        };
        active.last_cursor = cursor_physical;
        if delta_px == 0.0 {
            continue;
        }
        let delta_frac = delta_px / active.axis_extent;
        let mut next = layout.0.clone();
        if next.resize_split_at(&active.path, delta_frac, MIN_PANE_FRAC) {
            layout.0 = next;
        }
    }
}

/// Converts a divider's PHYSICAL-px `rect` into the LOGICAL-px `Val::Px`
/// fields of an absolute-position `Node`, mirroring
/// `crate::multiplexer::pane::layout::apply_rect`'s conversion for panes.
fn handle_node_rect(rect: PaneRect, inverse_scale_factor: f32) -> (Val, Val, Val, Val) {
    (
        Val::Px(rect.x * inverse_scale_factor),
        Val::Px(rect.y * inverse_scale_factor),
        Val::Px(rect.w * inverse_scale_factor),
        Val::Px(rect.h * inverse_scale_factor),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::MultiplexerLayout;
    use crate::multiplexer::window::MultiplexerWindow;

    /// Spawns an active window with a 2-leaf vertical split (`pane_a` left,
    /// `pane_b` right) plus a `WindowContainer` carrying a `ComputedNode` of
    /// `area_size` with `inverse_scale_factor: 1.0` (so physical == logical
    /// px, keeping test arithmetic exact). Returns
    /// `(window, container, pane_a, pane_b)`.
    fn spawn_active_window(app: &mut App, area_size: Vec2) -> (Entity, Entity, Entity, Entity) {
        let world = app.world_mut();
        let pane_a = world.spawn_empty().id();
        let pane_b = world.spawn_empty().id();
        let mut layout = MultiplexerLayout::new(pane_a);
        layout.split(pane_a, pane_b, SplitAxis::Vertical);
        let window = world
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(layout),
            ))
            .id();
        let container = world
            .spawn((
                WindowContainer { window },
                ComputedNode {
                    size: area_size,
                    inverse_scale_factor: 1.0,
                    ..ComputedNode::DEFAULT
                },
            ))
            .id();
        (window, container, pane_a, pane_b)
    }

    fn spawn_primary_window(app: &mut App, cursor: Option<Vec2>) -> Entity {
        let mut window = Window::default();
        window.set_cursor_position(cursor);
        app.world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Default),
            ))
            .id()
    }

    #[test]
    fn reconcile_spawns_one_handle_per_divider() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_systems(
            Update,
            reconcile_divider_handles
                .run_if(window_container_computed_changed.or_else(active_layout_tree_changed)),
        );
        let (window, container, _pane_a, pane_b) =
            spawn_active_window(&mut app, Vec2::new(101.0, 50.0));
        app.update();

        let count = |app: &mut App| {
            app.world_mut()
                .query::<&DividerHandle>()
                .iter(app.world())
                .count()
        };
        assert_eq!(count(&mut app), 1, "one handle for the single divider");
        assert!(
            app.world_mut()
                .query::<(&DividerHandle, &ChildOf)>()
                .iter(app.world())
                .all(|(h, child_of)| h.window == window && child_of.parent() == container),
            "handles belong to the active window and are parented to its container"
        );

        let pane_c = app.world_mut().spawn_empty().id();
        app.world_mut()
            .get_mut::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .split(pane_b, pane_c, SplitAxis::Horizontal);
        app.update();
        assert_eq!(
            count(&mut app),
            2,
            "reconciled to the new (nested) divider set"
        );

        app.world_mut()
            .get_mut::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .set_zoom(Some(pane_c));
        app.update();
        assert_eq!(count(&mut app), 0, "zoom hides every divider");
    }

    #[test]
    fn drag_changes_split_ratio() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<DividerDrag>()
            .add_systems(Update, divider_drag);
        let (window, _container, pane_a, pane_b) =
            spawn_active_window(&mut app, Vec2::new(101.0, 50.0));
        let primary = spawn_primary_window(&mut app, Some(Vec2::new(50.0, 25.0)));

        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let before = app
            .world()
            .get::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .rects(area, PANE_GAP_PX);
        let left_before = before.iter().find(|(e, _)| *e == pane_a).unwrap().1.w;

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window: primary,
        });
        app.update();

        app.world_mut().write_message(CursorMoved {
            window: primary,
            position: Vec2::new(60.0, 25.0),
            delta: None,
        });
        app.update();

        let after = app
            .world()
            .get::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .rects(area, PANE_GAP_PX);
        let left_after = after.iter().find(|(e, _)| *e == pane_a).unwrap().1.w;
        let right_after = after.iter().find(|(e, _)| *e == pane_b).unwrap().1.w;

        assert!(
            left_after > left_before,
            "dragging the vertical divider right must grow the first (left) child: {left_before} -> {left_after}"
        );
        assert_eq!(
            left_after, 60.0,
            "a 10px drag over a 101px/gap-1 area (axis_extent 101) grows the \
             ratio by 10/101 -> fw = round(100 * 0.599) = 60"
        );
        assert_eq!(left_after + right_after, 100.0, "gap-1 area covers 100px");
    }

    #[test]
    fn drag_zero_delta_does_not_trigger_change_detection() {
        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        fn layout_changed(
            windows: Query<
                (),
                (
                    With<ActiveMultiplexerWindow>,
                    Changed<MultiplexerLayoutComp>,
                ),
            >,
        ) -> bool {
            !windows.is_empty()
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<DividerDrag>()
            .init_resource::<RunCount>()
            .add_systems(Update, (divider_drag, probe.run_if(layout_changed)).chain());
        let (_window, _container, _pane_a, _pane_b) =
            spawn_active_window(&mut app, Vec2::new(101.0, 50.0));
        let primary = spawn_primary_window(&mut app, Some(Vec2::new(50.0, 25.0)));

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerLayoutComp counts as changed"
        );

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window: primary,
        });
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "starting a drag (no CursorMoved yet) must not trip Changed<MultiplexerLayoutComp>"
        );

        app.world_mut().write_message(CursorMoved {
            window: primary,
            position: Vec2::new(50.0, 25.0),
            delta: None,
        });
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a zero-delta CursorMoved must not spuriously trip Changed<MultiplexerLayoutComp>"
        );
    }

    #[test]
    fn hover_over_divider_sets_resize_cursor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, divider_hover_feedback);
        spawn_active_window(&mut app, Vec2::new(101.0, 50.0));
        let primary = spawn_primary_window(&mut app, Some(Vec2::new(50.0, 25.0)));

        app.update();

        let icon = app.world().entity(primary).get::<CursorIcon>();
        assert_eq!(
            icon,
            Some(&CursorIcon::System(SystemCursorIcon::ColResize)),
            "hovering a vertical divider must set the ColResize cursor"
        );
    }
}
