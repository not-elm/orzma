//! Visible pane-divider handle bars, the resize hover cursor, and mouse-drag
//! pane resize, built on `crate::multiplexer::layout`'s pure geometry
//! (`MultiplexerLayout::divider_rects` / `resize_split_at`, `divider_at`,
//! `DividerRect` / `ChildSide` — Task 14a).
//!
//! Three systems, self-contained in this one file (a `ui` module may freely
//! read mouse messages): `reconcile_divider_handles` spawns/despawns one
//! handle bar per divider of the active window, parented to its
//! `WindowContainer` and positioned from the same physical-px geometry
//! `crate::multiplexer::pane::layout::apply_layout` uses for panes — like
//! that system, it runs in `PostUpdate.after(UiSystems::Layout)` so it reads
//! the same-frame `ComputedNode`, not a frame-stale one;
//! `divider_hover_feedback` brightens the hovered handle and sets a
//! `ColResize` / `RowResize` cursor, running after `InputPhase::Hover` so it
//! overrides the hyperlink baseline cursor; `divider_drag` turns a Left
//! press-drag-release cycle over a divider into `resize_split_at` calls.

use crate::input::InputPhase;
use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::layout::{ChildSide, MultiplexerLayout, PaneRect, SplitAxis, divider_at};
use crate::multiplexer::pane::MIN_PANE_FRAC;
use crate::multiplexer::pane::layout::{
    PANE_GAP_PX, active_layout_tree_changed, rect_logical_vals, window_container_computed_changed,
};
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerLayoutComp};
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseMotion};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform, UiSystems};
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

/// Registers the divider-handle visuals, the resize hover cursor, and
/// mouse-drag pane resize.
///
/// The mouse/keyboard messages are normally registered by bevy's
/// `InputPlugin` / `WindowPlugin`; they are re-registered here (idempotent
/// `add_message`) so this file's `MessageReader`s and `on_message` run
/// conditions have a `Messages<T>` resource to read even when this plugin is
/// exercised without those plugins, mirroring `PanePlugin`.
pub(super) struct DividerHandlePlugin;

impl Plugin for DividerHandlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DividerDrag>()
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .add_message::<MouseMotion>()
            .add_message::<KeyboardInput>()
            .add_systems(
                PostUpdate,
                reconcile_divider_handles
                    .after(UiSystems::Layout)
                    .run_if(window_container_computed_changed.or_else(active_layout_tree_changed)),
            )
            .add_systems(
                Update,
                (
                    // Gated to the frames where hover state can actually
                    // change: the same message set that re-runs the baseline
                    // cursor system it overrides (`hyperlink_hover_and_cursor`
                    // — mouse motion, presses, key chords), plus the frames
                    // where the dividers themselves moved under a stationary
                    // cursor. An idle frame does no divider-rect work at all.
                    divider_hover_feedback.after(InputPhase::Hover).run_if(
                        on_message::<CursorMoved>
                            .or_else(on_message::<MouseMotion>)
                            .or_else(on_message::<MouseButtonInput>)
                            .or_else(on_message::<KeyboardInput>)
                            .or_else(active_layout_tree_changed)
                            .or_else(window_container_computed_changed),
                    ),
                    // NOTE: `.before(InputPhase::Dispatch)` so a press that
                    // starts a drag is reflected in `DividerDrag` before the
                    // mouse dispatchers read the same press and gate on
                    // `DividerDrag::claims`.
                    divider_drag
                        .before(InputPhase::Dispatch)
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

/// One in-progress divider drag: the owning window plus the target split's
/// path/axis/extent (captured at press time) and the last physical-px cursor
/// position along the split's axis, used to compute each `CursorMoved`'s
/// incremental delta. `window` lets `divider_drag` cancel the drag when the
/// active window changes mid-drag, so the captured path is never applied to
/// a different window's tree.
struct ActiveDrag {
    window: Entity,
    path: Vec<ChildSide>,
    axis: SplitAxis,
    axis_extent: f32,
    last_cursor: Vec2,
}

/// The in-progress divider drag, if any; `None` outside a Left
/// press-drag-release cycle.
///
/// `pub(crate)`: every Dispatch-phase mouse consumer gates on
/// [`DividerDrag::claims`] so a press inside a divider's ±grab-zone — which
/// lies INSIDE the adjacent panes' rects — resizes the divider without also
/// starting a selection, moving pane focus, or leaking press reports into
/// the pane (or its webview) under the cursor.
#[derive(Resource, Default)]
pub(crate) struct DividerDrag(Option<ActiveDrag>);

impl DividerDrag {
    /// Whether this in-progress drag claims `ev` — a Left press while a
    /// divider resize owns the pointer. `divider_drag` runs
    /// `.before(InputPhase::Dispatch)`, so by the time a dispatcher reads
    /// the same press the claim is already recorded; skipping the press
    /// keeps the consumer's gesture un-held, which lets the drag's motion
    /// and release die on its no-held paths without further checks.
    pub(crate) fn claims(&self, ev: &MouseButtonInput) -> bool {
        self.0.is_some() && ev.button == MouseButton::Left && ev.state == ButtonState::Pressed
    }

    /// Builds an in-progress drag on `window` for suppression tests.
    #[cfg(test)]
    pub(crate) fn dragging_for_test(window: Entity) -> Self {
        Self(Some(ActiveDrag {
            window,
            path: Vec::new(),
            axis: SplitAxis::Vertical,
            axis_extent: 1.0,
            last_cursor: Vec2::ZERO,
        }))
    }
}

/// Despawns and respawns the active window's handle bars from its current
/// `divider_rects`, gated to frames where the active layout tree or the
/// container's computed size changed (the two run conditions above). Runs in
/// `PostUpdate.after(UiSystems::Layout)`, matching `apply_layout`'s
/// scheduling — reading `ComputedNode` from `Update` would be one frame stale
/// during a live resize. Each handle is positioned with the same
/// physical-px-times-`inverse_scale_factor` conversion `apply_layout` uses
/// for pane rects, and carries a `GlobalZIndex` so hover-brighten renders
/// over pane content.
fn reconcile_divider_handles(
    mut commands: Commands,
    mut handles: Query<(Entity, &DividerHandle, &mut Node)>,
    active_windows: Query<(Entity, &MultiplexerLayoutComp), With<ActiveMultiplexerWindow>>,
    containers: Query<(Entity, &WindowContainer, &ComputedNode)>,
) {
    let Ok((window, layout)) = active_windows.single() else {
        return;
    };
    let Some((container, _, computed)) = containers.iter().find(|(_, c, _)| c.window == window)
    else {
        return;
    };
    let area = PaneRect::from_size(computed.size);
    let dividers = layout.0.divider_rects(area, PANE_GAP_PX);

    // A pure move/resize (mouse drag, keyboard resize, live window resize)
    // keeps the divider SET intact — update the existing bars' rects in
    // place instead of despawning and respawning N entities per frame of the
    // interaction. Indices were assigned 0..len at spawn, so a matching
    // count means a one-to-one mapping.
    let mut mine: Vec<(Entity, usize)> = handles
        .iter()
        .filter(|(_, handle, _)| handle.window == window)
        .map(|(entity, handle, _)| (entity, handle.index))
        .collect();
    if mine.len() == dividers.len() && !mine.is_empty() {
        mine.sort_by_key(|(_, index)| *index);
        for (entity, index) in mine {
            let Ok((_, _, mut node)) = handles.get_mut(entity) else {
                continue;
            };
            let (left, top, width, height) =
                rect_logical_vals(dividers[index].rect, computed.inverse_scale_factor);
            if node.left != left {
                node.left = left;
            }
            if node.top != top {
                node.top = top;
            }
            if node.width != width {
                node.width = width;
            }
            if node.height != height {
                node.height = height;
            }
        }
        return;
    }

    for (entity, handle, _) in handles.iter() {
        if handle.window == window {
            commands.entity(entity).despawn();
        }
    }
    for (index, divider) in dividers.iter().enumerate() {
        let (left, top, width, height) =
            rect_logical_vals(divider.rect, computed.inverse_scale_factor);
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
/// **Px-space:** `divider_rects` is PHYSICAL px in CONTAINER coordinates;
/// the window's cursor position is LOGICAL px in window coordinates.
/// `cursor_in_container` converts between them — the same conversion
/// `divider_drag` uses.
fn divider_hover_feedback(
    mut handles: Query<(&DividerHandle, &mut BackgroundColor)>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    active_windows: Query<(Entity, &MultiplexerLayoutComp), With<ActiveMultiplexerWindow>>,
    containers: Query<(&WindowContainer, &ComputedNode, &UiGlobalTransform)>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((window, layout)) = active_windows.single() else {
        return;
    };
    let Some((_, computed, transform)) = containers.iter().find(|(c, _, _)| c.window == window)
    else {
        return;
    };
    let Ok(win) = primary_window.single() else {
        return;
    };
    let area = PaneRect::from_size(computed.size);
    let cursor_logical = win.cursor_position();
    let dividers = match cursor_logical {
        Some(_) => layout.0.divider_rects(area, PANE_GAP_PX),
        None => Vec::new(),
    };
    let hovered = cursor_logical.and_then(|cursor_logical| {
        let cursor_physical = cursor_in_container(cursor_logical, computed, transform);
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
    containers: Query<(&WindowContainer, &ComputedNode, &UiGlobalTransform)>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((window, mut layout)) = windows.single_mut() else {
        buttons.clear();
        cursor_moved.clear();
        return;
    };
    let Some((_, computed, transform)) = containers.iter().find(|(c, _, _)| c.window == window)
    else {
        buttons.clear();
        cursor_moved.clear();
        return;
    };
    let area = PaneRect::from_size(computed.size);

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
                let cursor_physical = cursor_in_container(cursor_logical, computed, transform);
                let dividers = layout.0.divider_rects(area, PANE_GAP_PX);
                if let Some(index) = divider_at(&dividers, cursor_physical, DIVIDER_GRAB_TOL_PX) {
                    let hit = &dividers[index];
                    drag.0 = Some(ActiveDrag {
                        window,
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
    if active.window != window {
        drag.0 = None;
        cursor_moved.clear();
        return;
    }
    let mut next: Option<MultiplexerLayout> = None;
    let mut changed = false;
    for ev in cursor_moved.read() {
        let cursor_physical = cursor_in_container(ev.position, computed, transform);
        let delta_px = match active.axis {
            SplitAxis::Vertical => cursor_physical.x - active.last_cursor.x,
            SplitAxis::Horizontal => cursor_physical.y - active.last_cursor.y,
        };
        active.last_cursor = cursor_physical;
        if delta_px == 0.0 {
            continue;
        }
        let delta_frac = delta_px / active.axis_extent;
        let tree = next.get_or_insert_with(|| layout.0.clone());
        changed |= tree.resize_split_at(&active.path, active.axis, delta_frac, MIN_PANE_FRAC);
    }
    if changed && let Some(tree) = next {
        layout.0 = tree;
    }
}

/// Converts the primary window's LOGICAL cursor position into the window
/// container's PHYSICAL coordinate space — the space `divider_rects` computes
/// in: scale to physical px, then subtract the container's top-left.
///
/// `UiGlobalTransform.translation` is the node's CENTER in physical px (see
/// the anchor NOTE in `src/input/ime.rs`); the container's top-left is
/// `translation - size / 2`. Without this offset a container that sits below
/// the window bar (every one of them: the bar is 24 logical px tall) gets
/// grab zones displaced upward by the bar height.
fn cursor_in_container(
    cursor_logical: Vec2,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
) -> Vec2 {
    let origin = transform.translation - 0.5 * computed.size;
    cursor_logical / computed.inverse_scale_factor - origin
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::MultiplexerLayout;
    use crate::multiplexer::window::MultiplexerWindow;

    /// Spawns an active window with a 2-leaf `axis` split (`pane_a` first,
    /// `pane_b` second) plus a `WindowContainer` carrying a `ComputedNode` of
    /// `area_size` with `inverse_scale_factor: 1.0` (so physical == logical
    /// px, keeping test arithmetic exact), whose top-left sits at `origin` in
    /// window space (`UiGlobalTransform` holds the node's CENTER). `active`
    /// controls the `ActiveMultiplexerWindow` marker. Returns
    /// `(window, container, pane_a, pane_b)`.
    fn spawn_window_at(
        app: &mut App,
        area_size: Vec2,
        origin: Vec2,
        axis: SplitAxis,
        active: bool,
    ) -> (Entity, Entity, Entity, Entity) {
        let world = app.world_mut();
        let pane_a = world.spawn_empty().id();
        let pane_b = world.spawn_empty().id();
        let mut layout = MultiplexerLayout::new(pane_a);
        layout.split(pane_a, pane_b, axis);
        let window = world
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                MultiplexerLayoutComp(layout),
            ))
            .id();
        if active {
            world.entity_mut(window).insert(ActiveMultiplexerWindow);
        }
        let container = world
            .spawn((
                WindowContainer { window },
                ComputedNode {
                    size: area_size,
                    inverse_scale_factor: 1.0,
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(
                    origin.x + area_size.x / 2.0,
                    origin.y + area_size.y / 2.0,
                ),
            ))
            .id();
        (window, container, pane_a, pane_b)
    }

    /// `spawn_window_at` at the window origin with a vertical split, active.
    fn spawn_active_window(app: &mut App, area_size: Vec2) -> (Entity, Entity, Entity, Entity) {
        spawn_window_at(app, area_size, Vec2::ZERO, SplitAxis::Vertical, true)
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
            PostUpdate,
            reconcile_divider_handles
                .after(UiSystems::Layout)
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

        // A pure ratio change keeps the divider set: the existing handle is
        // updated in place (same entity), not despawned and respawned.
        let handle_before = app
            .world_mut()
            .query_filtered::<Entity, With<DividerHandle>>()
            .single(app.world())
            .unwrap();
        let left_before = app.world().get::<Node>(handle_before).unwrap().left;
        app.world_mut()
            .get_mut::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .resize_split_at(&[], SplitAxis::Vertical, 0.1, 0.05);
        app.update();
        let handle_after = app
            .world_mut()
            .query_filtered::<Entity, With<DividerHandle>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            handle_after, handle_before,
            "a move-only reconcile must reuse the existing handle entity"
        );
        assert_ne!(
            app.world().get::<Node>(handle_after).unwrap().left,
            left_before,
            "the reused handle must be repositioned to the new divider rect"
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

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<DividerDrag>()
            .init_resource::<RunCount>()
            .add_systems(
                Update,
                (divider_drag, probe.run_if(active_layout_tree_changed)).chain(),
            );
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

    #[test]
    fn hover_accounts_for_the_container_offset() {
        // The WindowContainer sits 24px below the window top (the window
        // bar). A horizontal split in a 100x50 container puts the divider at
        // container-y ~25, i.e. on-screen y ~49.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, divider_hover_feedback);
        spawn_window_at(
            &mut app,
            Vec2::new(100.0, 50.0),
            Vec2::new(0.0, 24.0),
            SplitAxis::Horizontal,
            true,
        );
        // Cursor over the window bar, where the un-offset math would
        // (wrongly) place the divider's grab zone.
        let primary = spawn_primary_window(&mut app, Some(Vec2::new(50.0, 25.0)));
        app.update();
        assert_eq!(
            app.world().entity(primary).get::<CursorIcon>(),
            Some(&CursorIcon::System(SystemCursorIcon::Default)),
            "a cursor over the window bar must not read as hovering the divider"
        );

        // Cursor on the divider's actual on-screen position.
        app.world_mut()
            .get_mut::<Window>(primary)
            .unwrap()
            .set_cursor_position(Some(Vec2::new(50.0, 49.0)));
        app.update();
        assert_eq!(
            app.world().entity(primary).get::<CursorIcon>(),
            Some(&CursorIcon::System(SystemCursorIcon::RowResize)),
            "the divider's visible position must be hoverable despite the container offset"
        );
    }

    #[test]
    fn drag_does_not_apply_to_a_window_activated_mid_drag() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<DividerDrag>()
            .add_systems(Update, divider_drag);
        let (window_0, _, _, _) = spawn_active_window(&mut app, Vec2::new(101.0, 50.0));
        let (window_1, _, pane_1a, _) = spawn_window_at(
            &mut app,
            Vec2::new(101.0, 50.0),
            Vec2::ZERO,
            SplitAxis::Vertical,
            false,
        );
        let primary = spawn_primary_window(&mut app, Some(Vec2::new(50.0, 25.0)));

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window: primary,
        });
        app.update();

        // A window switch lands mid-drag (button still held).
        app.world_mut()
            .entity_mut(window_0)
            .remove::<ActiveMultiplexerWindow>();
        app.world_mut()
            .entity_mut(window_1)
            .insert(ActiveMultiplexerWindow);

        app.world_mut().write_message(CursorMoved {
            window: primary,
            position: Vec2::new(60.0, 25.0),
            delta: None,
        });
        app.update();

        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rects = app
            .world()
            .get::<MultiplexerLayoutComp>(window_1)
            .unwrap()
            .0
            .rects(area, PANE_GAP_PX);
        let first = rects.iter().find(|(e, _)| *e == pane_1a).unwrap().1;
        assert_eq!(
            first.w, 50.0,
            "a drag captured on window 0 must not resize window 1's split"
        );
    }
}
