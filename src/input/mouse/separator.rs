//! Grabbing and dragging the divider between two panes.

use super::{TerminalSurfaces, on_any_mouse_message};
use crate::input::mouse::MousePhase;
use crate::surface::geometry::phys_to_pane_local;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_orzmux::prelude::{
    OrzmuxPaneContainer, OrzmuxSeparator, PaneGeometry, RequestSplitResize, SplitId,
    SplitOrientation,
};

pub(super) struct SeparatorDragPlugin;

impl Plugin for SeparatorDragPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                drive_separator_drag
                    .in_set(MousePhase::Grab)
                    .run_if(on_any_mouse_message().or_else(any_with_component::<GrabbedSeparator>)),
                retire_separator_drag
                    .in_set(MousePhase::Retire)
                    .run_if(any_with_component::<GrabbedSeparator>),
            ),
        );
    }
}

/// Every pane divider, with the geometry a grab band is measured
/// against.
pub(in crate::input) type SeparatorNodes<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static OrzmuxSeparator,
        &'static ComputedNode,
        &'static UiGlobalTransform,
    ),
>;

/// The divider whose grab band contains a cursor position.
pub(crate) struct SeparatorHit {
    /// The separator entity the band belongs to.
    pub(crate) entity: Entity,
    /// The split the divider moves.
    pub(crate) split: SplitId,
    /// Which way the divider runs.
    pub(crate) orientation: SplitOrientation,
}

impl SeparatorHit {
    /// The divider whose grab band contains `cursor_phys`, in window
    /// physical px. `scale` is the window's scale factor, physical px
    /// per logical px. Overlapping bands resolve to the nearer painted
    /// line, and an exact tie to the lower [`SplitId`]. `cell_px` is the
    /// `(width, height)` cell pitch in physical px. A cursor on the
    /// line's own axis but past its painted end returns `None`.
    pub(crate) fn resolve<'a>(
        cursor_phys: Vec2,
        scale: f32,
        cell_px: (f32, f32),
        separators: impl Iterator<
            Item = (
                Entity,
                &'a OrzmuxSeparator,
                &'a ComputedNode,
                &'a UiGlobalTransform,
            ),
        >,
    ) -> Option<Self> {
        let mut best: Option<(f32, SplitId, Self)> = None;
        for (entity, marker, node, transform) in separators {
            let centre = transform.translation;
            let pitch = match marker.orientation {
                SplitOrientation::Vertical => cell_px.0,
                SplitOrientation::Horizontal => cell_px.1,
            };
            let half_band = grab_half_band_phys(scale, pitch);
            let (across, along, half_len) = match marker.orientation {
                SplitOrientation::Vertical => (
                    (cursor_phys.x - centre.x).abs(),
                    (cursor_phys.y - centre.y).abs(),
                    node.size.y / 2.0,
                ),
                SplitOrientation::Horizontal => (
                    (cursor_phys.y - centre.y).abs(),
                    (cursor_phys.x - centre.x).abs(),
                    node.size.x / 2.0,
                ),
            };
            if across > half_band || along > half_len {
                continue;
            }
            let candidate = Self {
                entity,
                split: marker.split,
                orientation: marker.orientation,
            };
            let better = match &best {
                None => true,
                Some((best_across, best_split, _)) => {
                    across < *best_across || (across == *best_across && marker.split < *best_split)
                }
            };
            if better {
                best = Some((across, marker.split, candidate));
            }
        }
        best.map(|(_, _, hit)| hit)
    }

    /// The divider whose grab band contains `cursor_phys`, in window
    /// physical px, measured against the cell pitch and scale factor
    /// `geometry` records.
    pub(in crate::input) fn at<'a>(
        cursor_phys: Vec2,
        geometry: &PaneGeometry,
        separators: impl Iterator<
            Item = (
                Entity,
                &'a OrzmuxSeparator,
                &'a ComputedNode,
                &'a UiGlobalTransform,
            ),
        >,
    ) -> Option<Self> {
        Self::resolve(
            cursor_phys,
            geometry.scale_factor,
            geometry.cell_pitch(),
            separators,
        )
    }
}

/// The divider the pointer is holding. While it exists, every other
/// mouse consumer forwards nothing and keeps whatever in-flight state it
/// already holds.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(in crate::input) struct GrabbedSeparator {
    /// The split the held divider moves.
    split: SplitId,
    /// Which way the held divider runs.
    orientation: SplitOrientation,
    /// The last position sent, so an unchanged boundary sends nothing.
    last_sent: u16,
    /// The last reported cursor in window physical px, so a drag that
    /// leaves the window keeps extending from where the pointer was.
    last_cursor_phys: Vec2,
    /// True once the release or a focus loss was seen, which ends the
    /// grab at the end of that frame.
    releasing: bool,
}

#[cfg(test)]
impl GrabbedSeparator {
    /// A grab on `split`, whose divider runs `orientation`, seeded as if
    /// the pointer had not moved since the press.
    pub(in crate::input) fn held(split: SplitId, orientation: SplitOrientation) -> Self {
        Self {
            split,
            orientation,
            last_sent: 0,
            last_cursor_phys: Vec2::ZERO,
            releasing: false,
        }
    }
}

/// Half the grab band's thickness in logical px, measured from the
/// painted line's centre.
///
/// TODO: make the grab band configurable.
const SEPARATOR_GRAB_HALF_BAND_LOGICAL_PX: f32 = 4.0;

/// Half the grab band in physical px: never below
/// [`SEPARATOR_GRAB_HALF_BAND_LOGICAL_PX`] logical px, and never below
/// half a cell.
fn grab_half_band_phys(scale: f32, cell_pitch_phys: f32) -> f32 {
    (SEPARATOR_GRAB_HALF_BAND_LOGICAL_PX * scale).max(cell_pitch_phys / 2.0)
}

fn drive_separator_drag(
    mut commands: Commands,
    mut buttons: MessageReader<MouseButtonInput>,
    mut cursor_moved: MessageReader<CursorMoved>,
    mut grabbed: Query<&mut GrabbedSeparator>,
    separators: SeparatorNodes,
    container: Query<(&ComputedNode, &UiGlobalTransform), With<OrzmuxPaneContainer>>,
    terminals: TerminalSurfaces,
    geometry: Option<Res<PaneGeometry>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(geometry) = geometry else {
        buttons.clear();
        cursor_moved.clear();
        return;
    };
    let cell_px = geometry.cell_pitch();
    let window = windows.single().ok();
    let focused = window.is_some_and(|window| window.focused);
    let cursor = reported_cursor_phys(window, cursor_moved.read().last());
    let (pressed, released) = left_button_edges(buttons.read());

    if let Ok(mut grab) = grabbed.single_mut() {
        let cursor = cursor.unwrap_or(grab.last_cursor_phys);
        let mut next = *grab;
        next.last_cursor_phys = cursor;
        next.releasing |= released || !focused;
        if let Some(local) = container_local(&container, cursor) {
            let position = boundary_at(local, cell_px, next.orientation);
            if position != next.last_sent {
                next.last_sent = position;
                commands.trigger(RequestSplitResize {
                    split: next.split,
                    position,
                });
            }
        }
        grab.set_if_neq(next);
        return;
    }

    if !pressed || terminals.is_empty() {
        return;
    }
    let Some(cursor) = cursor else {
        return;
    };
    let Some(hit) = SeparatorHit::at(cursor, &geometry, separators.iter()) else {
        return;
    };
    let Some(local) = container_local(&container, cursor) else {
        return;
    };
    // NOTE: a press and its release can share one message batch, and the
    // insert below is only queued, so the grab branch above does not run
    // on such a frame. Seeding `releasing` here is what ends that grab.
    commands.entity(hit.entity).insert(GrabbedSeparator {
        split: hit.split,
        orientation: hit.orientation,
        last_sent: boundary_at(local, cell_px, hit.orientation),
        last_cursor_phys: cursor,
        releasing: released,
    });
}

fn retire_separator_drag(mut commands: Commands, grabbed: Query<(Entity, &GrabbedSeparator)>) {
    for (entity, grab) in &grabbed {
        if grab.releasing {
            commands.entity(entity).remove::<GrabbedSeparator>();
        }
    }
}

/// The pointer in window physical px, preferring the window's retained
/// position over the frame's own `CursorMoved`, which is reported in
/// logical px. The retained position is `None` while the pointer is
/// outside the client area.
fn reported_cursor_phys(window: Option<&Window>, moved: Option<&CursorMoved>) -> Option<Vec2> {
    let scale = window.map_or(1.0, Window::scale_factor);
    window
        .and_then(Window::physical_cursor_position)
        .or_else(|| moved.map(|moved| moved.position * scale))
}

/// Whether the left button was pressed and whether it was released
/// anywhere in `buttons`; both are true when one batch carries both.
fn left_button_edges<'a>(buttons: impl Iterator<Item = &'a MouseButtonInput>) -> (bool, bool) {
    let mut pressed = false;
    let mut released = false;
    for ev in buttons {
        if ev.button != MouseButton::Left {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => pressed = true,
            ButtonState::Released => released = true,
        }
    }
    (pressed, released)
}

/// The whole-window cell whose far edge is the boundary nearest
/// `container_local`, a position in the pane container's local physical
/// px. `cell_px` is the `(width, height)` cell pitch in physical px. A
/// position at or before the container's origin yields zero.
fn boundary_at(container_local: Vec2, cell_px: (f32, f32), orientation: SplitOrientation) -> u16 {
    let (offset, pitch) = match orientation {
        SplitOrientation::Vertical => (container_local.x, cell_px.0),
        SplitOrientation::Horizontal => (container_local.y, cell_px.1),
    };
    let boundary = (offset / pitch).round().max(0.0) as u16;
    boundary.saturating_sub(1)
}

/// `cursor_phys` in the pane container's local physical px, or `None`
/// when no container is projectable.
fn container_local(
    container: &Query<(&ComputedNode, &UiGlobalTransform), With<OrzmuxPaneContainer>>,
    cursor_phys: Vec2,
) -> Option<Vec2> {
    let (node, transform) = container.single().ok()?;
    phys_to_pane_local(node, transform, cursor_phys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mouse::MouseEffect;
    use crate::input::mouse::MouseInputPlugin;
    use crate::input::mouse::gesture::OrzmaMouseGesture;
    use crate::input::mouse::test_support::{
        CapturedEffects, add_effect_capture_observers, set_phys_cursor, test_metrics,
    };
    use crate::input::mouse::webview::WebviewPress;
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::message::Messages;
    use bevy::input::mouse::MouseWheel;
    use bevy::window::{WindowFocused, WindowResolution};
    use bevy_orzma_tty_renderer::schema::TerminalView;
    use bevy_orzmux::prelude::RequestTtyPointer;
    use orzma_tty::CellPixels;
    use orzma_tty::prelude::{PointerInput, PointerKind};

    const SCALE: f32 = 1.0;
    const CELL: (f32, f32) = (8.0, 16.0);

    /// A vertical divider one physical px wide, centred at `x`, running
    /// from y=0 to y=`len_px`.
    fn vertical(x: f32, len_px: f32) -> (ComputedNode, UiGlobalTransform) {
        (
            ComputedNode {
                size: Vec2::new(1.0, len_px),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(x, len_px / 2.0),
        )
    }

    /// A horizontal divider one physical px tall, centred at `y`,
    /// running from x=0 to x=`len_px`.
    fn horizontal(y: f32, len_px: f32) -> (ComputedNode, UiGlobalTransform) {
        (
            ComputedNode {
                size: Vec2::new(len_px, 1.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(len_px / 2.0, y),
        )
    }

    /// Asserts that a press within the band around the painted line
    /// resolves to that divider, that one beyond the band does not, and
    /// that a press anywhere along the line's length still resolves.
    ///
    /// Case: the user aims at the visible groove between two panes, and
    /// then clicks well inside the pane next to it.
    #[test]
    fn a_press_inside_the_band_resolves_and_one_outside_does_not() {
        let marker = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (node, transform) = vertical(100.0, 400.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(103.0, 200.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert_eq!(hit.as_ref().map(|h| h.entity), Some(Entity::PLACEHOLDER));
        assert_eq!(hit.map(|h| h.split), Some(SplitId(1)));

        let miss = SeparatorHit::resolve(
            Vec2::new(140.0, 200.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert!(miss.is_none());

        let just_past_the_band = SeparatorHit::resolve(
            Vec2::new(106.0, 200.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert!(just_past_the_band.is_none());

        let along_the_line = SeparatorHit::resolve(
            Vec2::new(100.0, 250.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert_eq!(along_the_line.map(|h| h.split), Some(SplitId(1)));
    }

    /// Asserts that a press beyond the divider's own length misses it,
    /// so the band is bounded on both axes.
    ///
    /// Case: a short divider splits only the top half of the window and
    /// the user clicks in the full-height pane below it, on the same
    /// column the divider occupies.
    #[test]
    fn a_press_past_the_divider_length_misses_it() {
        let marker = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (node, transform) = vertical(100.0, 192.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(100.0, 320.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert!(hit.is_none());
    }

    /// Asserts that overlapping bands resolve to the nearer painted
    /// line, and that an exact tie resolves to the lower split id.
    ///
    /// Case: the user aims between two vertical column dividers that
    /// sit five physical px apart.
    #[test]
    fn overlapping_bands_resolve_to_the_nearer_line_then_the_lower_id() {
        let near = OrzmuxSeparator {
            split: SplitId(2),
            orientation: SplitOrientation::Vertical,
        };
        let far = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (near_node, near_transform) = vertical(100.0, 400.0);
        let (far_node, far_transform) = vertical(105.0, 400.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(101.0, 200.0),
            SCALE,
            CELL,
            [
                (Entity::PLACEHOLDER, &far, &far_node, &far_transform),
                (Entity::PLACEHOLDER, &near, &near_node, &near_transform),
            ]
            .into_iter(),
        );
        assert_eq!(hit.map(|h| h.split), Some(SplitId(2)));

        let (a_node, a_transform) = vertical(100.0, 400.0);
        let (b_node, b_transform) = vertical(100.0, 400.0);
        let tie = SeparatorHit::resolve(
            Vec2::new(100.0, 200.0),
            SCALE,
            CELL,
            [
                (Entity::PLACEHOLDER, &near, &a_node, &a_transform),
                (Entity::PLACEHOLDER, &far, &b_node, &b_transform),
            ]
            .into_iter(),
        );
        assert_eq!(tie.map(|h| h.split), Some(SplitId(1)));
    }

    /// Asserts that the grab band is the logical-px constant converted
    /// to physical px, so a high-DPI window gets the same band in
    /// logical terms rather than a halved one.
    ///
    /// Case: the user works on a Retina display, where every physical
    /// px the cursor reports is half a logical px wide.
    #[test]
    fn the_band_scales_with_the_window_scale_factor() {
        const RETINA_SCALE: f32 = 2.0;
        const SMALL_CELL: (f32, f32) = (4.0, 8.0);

        let marker = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (node, transform) = vertical(100.0, 400.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(106.0, 200.0),
            RETINA_SCALE,
            SMALL_CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert_eq!(hit.map(|h| h.split), Some(SplitId(1)));

        let miss = SeparatorHit::resolve(
            Vec2::new(109.0, 200.0),
            RETINA_SCALE,
            SMALL_CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert!(miss.is_none());
    }

    /// Asserts that a horizontal divider's band is measured across the
    /// cell height and bounded by the divider's own width, and that the
    /// hit reports its orientation.
    ///
    /// Case: two panes are stacked one above the other, and the user
    /// aims at the row divider between them before clicking past its
    /// right end in the full-width pane beside it.
    #[test]
    fn a_horizontal_divider_bands_across_its_own_axis() {
        let marker = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Horizontal,
        };
        let (node, transform) = horizontal(100.0, 400.0);
        let at = |cursor| {
            SeparatorHit::resolve(
                cursor,
                SCALE,
                CELL,
                [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
            )
        };

        assert_eq!(
            at(Vec2::new(200.0, 107.0)).map(|h| h.orientation),
            Some(SplitOrientation::Horizontal)
        );
        assert!(at(Vec2::new(200.0, 110.0)).is_none());
        assert_eq!(
            at(Vec2::new(350.0, 100.0)).map(|h| h.split),
            Some(SplitId(1))
        );
        assert!(at(Vec2::new(450.0, 100.0)).is_none());
    }

    #[derive(Resource, Default)]
    struct Requested(Vec<u16>);

    /// A focused window whose scale factor is `scale`, a pane container
    /// and a terminal surface filling it at an 8x16 logical px cell, the
    /// matching `PaneGeometry`, and a recorder for every
    /// `RequestSplitResize`. Registers no plugin, so each caller picks
    /// the one it wants to exercise. Every position the helpers below
    /// take is in window physical px.
    fn drag_world(scale: f32) -> App {
        let phys = Vec2::new(800.0, 400.0) * scale;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .init_resource::<Requested>()
            .insert_resource(PaneGeometry {
                cell_px: CellPixels {
                    width: (CELL.0 * scale) as u16,
                    height: (CELL.1 * scale) as u16,
                },
                scale_factor: scale,
            })
            .add_observer(|e: On<RequestSplitResize>, mut got: ResMut<Requested>| {
                got.0.push(e.position);
            });
        app.world_mut().spawn((
            OrzmaTerminal,
            ComputedNode {
                size: phys,
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(phys.x / 2.0, phys.y / 2.0),
            TerminalView {
                cols: 100,
                rows: 25,
                ..default()
            },
        ));
        app.world_mut().spawn((
            OrzmuxPaneContainer,
            ComputedNode {
                size: phys,
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(phys.x / 2.0, phys.y / 2.0),
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(phys.x as u32, phys.y as u32)
                    .with_scale_factor_override(scale),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    /// `drag_world` plus the real `SeparatorDragPlugin` alone, so a test
    /// sees the grab without any dispatcher competing for the same
    /// messages.
    fn drag_app(scale: f32) -> App {
        let mut app = drag_world(scale);
        app.add_plugins(SeparatorDragPlugin)
            .configure_sets(Update, (MousePhase::Grab, MousePhase::Retire).chain());
        app
    }

    /// `drag_world` plus the whole real `MouseInputPlugin`, so the
    /// `MousePhase` chain under test is the shipped one rather than a
    /// copy of it, sync points included.
    fn suppression_app() -> App {
        let mut app = drag_world(SCALE);
        app.add_plugins(MouseInputPlugin)
            .add_message::<MouseWheel>()
            .add_message::<WindowFocused>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<CapturedEffects>()
            .insert_resource(test_metrics());
        add_effect_capture_observers(&mut app);
        app
    }

    /// Spawns a vertical divider whose painted line is centred at
    /// `x_phys` and runs the full `len_px`.
    fn spawn_vertical_separator(app: &mut App, split: SplitId, x_phys: f32, len_px: f32) -> Entity {
        app.world_mut()
            .spawn((
                OrzmuxSeparator {
                    split,
                    orientation: SplitOrientation::Vertical,
                },
                ComputedNode {
                    size: Vec2::new(1.0, len_px),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(x_phys, len_px / 2.0),
            ))
            .id()
    }

    fn primary_window(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap()
    }

    fn window_scale(app: &mut App) -> f32 {
        let window = primary_window(app);
        app.world().get::<Window>(window).unwrap().scale_factor()
    }

    /// Writes the `CursorMoved` matching `phys` window physical px,
    /// converted to the logical px the message carries.
    fn write_cursor_moved(app: &mut App, phys: Vec2) {
        let window = primary_window(app);
        let position = phys / window_scale(app);
        app.world_mut()
            .resource_mut::<Messages<CursorMoved>>()
            .write(CursorMoved {
                window,
                position,
                delta: None,
            });
    }

    /// Places the pointer at `phys` window physical px, both on the
    /// window itself and as a `CursorMoved` message.
    fn set_cursor(app: &mut App, phys: Vec2) {
        set_phys_cursor(app, phys);
        write_cursor_moved(app, phys);
    }

    fn write_button(app: &mut App, button: MouseButton, state: ButtonState) {
        let window = primary_window(app);
        app.world_mut()
            .resource_mut::<Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button,
                state,
                window,
            });
    }

    fn write_left(app: &mut App, state: ButtonState) {
        write_button(app, MouseButton::Left, state);
    }

    fn press_at(app: &mut App, phys: Vec2) {
        set_cursor(app, phys);
        write_left(app, ButtonState::Pressed);
        app.update();
    }

    fn move_to(app: &mut App, phys: Vec2) {
        set_cursor(app, phys);
        app.update();
    }

    /// Places the pointer at `phys` window physical px when `phys` falls
    /// outside the window's resolution, which winit records on the window
    /// like any other position, so `Window::physical_cursor_position`
    /// bounds-checks it away and only the `CursorMoved` carries the
    /// pointer.
    ///
    /// # Panics
    ///
    /// Panics when `phys` is inside the window, where
    /// `Window::physical_cursor_position` would report it after all.
    fn move_off_window(app: &mut App, phys: Vec2) {
        let window = primary_window(app);
        let resolution = &app.world().get::<Window>(window).unwrap().resolution;
        debug_assert!(
            phys.x < 0.0
                || phys.y < 0.0
                || phys.x >= resolution.physical_width() as f32
                || phys.y >= resolution.physical_height() as f32,
            "move_off_window needs a point the window's bounds check rejects"
        );
        move_to(app, phys);
    }

    fn release(app: &mut App) {
        write_left(app, ButtonState::Released);
        app.update();
    }

    /// The positions requested since the last call, draining the record.
    fn requested(app: &mut App) -> Vec<u16> {
        std::mem::take(&mut app.world_mut().resource_mut::<Requested>().0)
    }

    /// Whether the dispatcher handed any pane a press or a release.
    fn pressed_a_pane(app: &App) -> bool {
        app.world()
            .resource::<CapturedEffects>()
            .0
            .iter()
            .any(|effect| {
                matches!(
                    effect,
                    MouseEffect::Pointer(PointerInput {
                        kind: PointerKind::Press | PointerKind::Release,
                        ..
                    })
                )
            })
    }

    /// Asserts that a cursor is converted to the nearest whole-window
    /// cell boundary, so a press anywhere in the band maps to the
    /// boundary the line is already on.
    ///
    /// Case: the divider between columns 39 and 40 is painted on their
    /// shared boundary, and the user presses a few px to either side of
    /// it.
    #[test]
    fn a_cursor_maps_to_the_nearest_cell_boundary() {
        assert_eq!(
            boundary_at(Vec2::new(320.0, 0.0), CELL, SplitOrientation::Vertical),
            39
        );
        assert_eq!(
            boundary_at(Vec2::new(317.0, 0.0), CELL, SplitOrientation::Vertical),
            39
        );
        assert_eq!(
            boundary_at(Vec2::new(323.0, 0.0), CELL, SplitOrientation::Vertical),
            39
        );
        assert_eq!(
            boundary_at(Vec2::new(329.0, 0.0), CELL, SplitOrientation::Vertical),
            40
        );
    }

    /// Asserts that a boundary at or before the window origin saturates
    /// to zero rather than wrapping.
    ///
    /// Case: the user drags a divider past the left edge of the window.
    #[test]
    fn a_boundary_before_the_origin_saturates_to_zero() {
        assert_eq!(
            boundary_at(Vec2::new(0.0, 0.0), CELL, SplitOrientation::Vertical),
            0
        );
        assert_eq!(
            boundary_at(Vec2::new(-40.0, 0.0), CELL, SplitOrientation::Vertical),
            0
        );
    }

    /// Asserts that a horizontal divider's boundary is measured down the
    /// cell height rather than across the cell width.
    ///
    /// Case: the user drags the row divider between two stacked panes.
    #[test]
    fn a_horizontal_divider_counts_boundaries_down_the_cell_height() {
        assert_eq!(
            boundary_at(Vec2::new(0.0, 320.0), CELL, SplitOrientation::Horizontal),
            19
        );
    }

    /// Asserts that a press on the band grabs the divider, a drag to a
    /// new boundary requests exactly one move, staying on that boundary
    /// requests nothing more, and the release retires the grab.
    ///
    /// Case: the user presses on the divider, drags it two cells to the
    /// right, jiggles the mouse without leaving that cell, and lets go.
    #[test]
    fn a_drag_requests_one_move_per_boundary_and_retires_on_release() {
        let mut app = drag_app(2.0);
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 640.0, 800.0);

        press_at(&mut app, Vec2::new(640.0, 400.0));
        assert!(app.world().get::<GrabbedSeparator>(separator).is_some());
        assert!(requested(&mut app).is_empty());

        move_to(&mut app, Vec2::new(672.0, 400.0));
        assert_eq!(requested(&mut app), vec![41]);

        move_to(&mut app, Vec2::new(676.0, 400.0));
        assert!(requested(&mut app).is_empty());

        release(&mut app);
        assert!(app.world().get::<GrabbedSeparator>(separator).is_none());
    }

    /// Asserts that a drag whose cursor leaves the window keeps moving
    /// the divider from the last reported pointer position.
    ///
    /// Case: the user drags a divider past the right edge of the window
    /// and the pointer leaves the client area.
    #[test]
    fn a_drag_continues_after_the_cursor_leaves_the_window() {
        let mut app = drag_app(SCALE);
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        press_at(&mut app, Vec2::new(320.0, 200.0));
        requested(&mut app);

        move_off_window(&mut app, Vec2::new(900.0, 200.0));

        assert!(app.world().get::<GrabbedSeparator>(separator).is_some());
        assert_eq!(requested(&mut app), vec![112]);
    }

    /// Asserts that a reported cursor is read as logical px and scaled to
    /// physical px before the boundary is computed, so a high-DPI drag
    /// lands on the boundary under the pointer rather than at half the
    /// offset.
    ///
    /// Case: the user works on a Retina display and drags a divider past
    /// the right edge of the window.
    #[test]
    fn a_reported_cursor_is_scaled_to_physical_px() {
        const RETINA_SCALE: f32 = 2.0;

        let mut app = drag_app(RETINA_SCALE);
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 640.0, 800.0);

        press_at(&mut app, Vec2::new(640.0, 400.0));
        assert!(app.world().get::<GrabbedSeparator>(separator).is_some());
        requested(&mut app);

        move_off_window(&mut app, Vec2::new(1800.0, 400.0));

        assert_eq!(requested(&mut app), vec![112]);
    }

    /// Asserts that a press and its release arriving in one message
    /// batch still retire the grab.
    ///
    /// Case: the user clicks a divider once without dragging it, and both
    /// button messages reach the same frame.
    #[test]
    fn a_press_and_release_in_one_frame_retires_the_grab() {
        let mut app = drag_app(SCALE);
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        set_cursor(&mut app, Vec2::new(320.0, 200.0));
        write_left(&mut app, ButtonState::Pressed);
        write_left(&mut app, ButtonState::Released);
        app.update();
        app.update();

        assert!(app.world().get::<GrabbedSeparator>(separator).is_none());
    }

    /// Asserts that a window losing focus retires the grab on a frame
    /// carrying no mouse message at all.
    ///
    /// Case: the user drags a divider and then switches to another
    /// application with the button still held.
    #[test]
    fn losing_window_focus_retires_the_grab() {
        let mut app = drag_app(SCALE);
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        press_at(&mut app, Vec2::new(320.0, 200.0));

        let window = primary_window(&mut app);
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        app.update();

        assert!(app.world().get::<GrabbedSeparator>(separator).is_none());
    }

    /// Asserts that a press a separator grab consumed hands no press to the
    /// pane beside the divider and leaves the shared gesture unlocked, on
    /// the press frame itself.
    ///
    /// Case: the user presses on the divider between two panes.
    #[test]
    fn a_grabbed_press_reaches_no_neighbouring_pane() {
        let mut app = suppression_app();
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        press_at(&mut app, Vec2::new(320.0, 200.0));

        assert!(app.world().get::<GrabbedSeparator>(separator).is_some());
        assert!(!pressed_a_pane(&app));
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that a press and its release arriving in one message batch
    /// suppress the button dispatcher for that frame and retire the grab
    /// by the end of it, so the click reaches no pane, locks no gesture, and
    /// is not replayed into a pane on a later frame.
    ///
    /// Case: the user clicks a divider once, quickly enough that the
    /// press and the release arrive in one frame's message batch.
    #[test]
    fn a_same_frame_click_on_a_divider_leaves_the_neighbour_alone() {
        let mut app = suppression_app();
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        set_cursor(&mut app, Vec2::new(320.0, 200.0));
        write_left(&mut app, ButtonState::Pressed);
        write_left(&mut app, ButtonState::Released);
        app.update();

        assert!(!pressed_a_pane(&app));
        assert!(app.world().get::<GrabbedSeparator>(separator).is_none());
        let gesture = app.world().resource::<OrzmaMouseGesture>();
        assert!(gesture.held.is_none());
        set_cursor(&mut app, Vec2::new(360.0, 200.0));
        app.update();
        assert!(
            !pressed_a_pane(&app),
            "a later frame must not replay the divider click into the pane"
        );
    }

    /// Asserts that a whole press-drag-release gesture on a divider hands
    /// no press or release to any pane on any of its frames.
    ///
    /// Case: the user drags a divider across two cells and lets go.
    #[test]
    fn a_whole_divider_drag_presses_no_pane() {
        let mut app = suppression_app();
        spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        press_at(&mut app, Vec2::new(320.0, 200.0));
        move_to(&mut app, Vec2::new(336.0, 200.0));
        release(&mut app);
        app.update();

        assert!(!pressed_a_pane(&app));
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that a separator grab cancels a gesture a pane button still
    /// holds, so the pane's application is not left with that button down.
    ///
    /// Case: the user holds the right button in nvim's pane, moves onto the
    /// divider and presses the left button there, and then lets go of the
    /// right button while the divider is still grabbed.
    #[test]
    fn a_grab_cancels_a_gesture_held_in_a_pane() {
        #[derive(Resource, Default)]
        struct Cancels(Vec<Entity>);
        let mut app = suppression_app();
        app.init_resource::<Cancels>().add_observer(
            |ev: On<RequestTtyPointer>, mut cancels: ResMut<Cancels>| {
                if ev.input.kind == PointerKind::Cancel {
                    cancels.0.push(ev.terminal);
                }
            },
        );
        let pane = app
            .world_mut()
            .query_filtered::<Entity, With<OrzmaTerminal>>()
            .single(app.world())
            .expect("drag_world spawns one terminal");
        let separator = spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        set_cursor(&mut app, Vec2::new(204.0, 200.0));
        write_button(&mut app, MouseButton::Right, ButtonState::Pressed);
        app.update();
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_some());
        press_at(&mut app, Vec2::new(322.0, 200.0));
        write_button(&mut app, MouseButton::Right, ButtonState::Released);
        app.update();

        assert!(app.world().get::<GrabbedSeparator>(separator).is_some());
        assert_eq!(app.world().resource::<Cancels>().0, vec![pane]);
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that the webview pointer router leaves an in-flight press
    /// recorded rather than releasing or clearing it, on a frame a
    /// separator grab consumed.
    ///
    /// Case: the user presses on a divider while an inline web page still
    /// holds the press it was given by an earlier click.
    #[test]
    fn a_grabbed_press_does_not_reach_the_webview_router() {
        let mut app = suppression_app();
        app.insert_resource(WebviewPress(Some(Entity::PLACEHOLDER)));
        spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        press_at(&mut app, Vec2::new(320.0, 200.0));

        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            Some(Entity::PLACEHOLDER)
        );
    }

    /// Asserts that a press landing outside every grab band still
    /// reaches the button dispatcher and the pane under it.
    ///
    /// Case: the user clicks in the middle of a pane, well away from the
    /// divider beside it.
    #[test]
    fn an_ungrabbed_press_still_reaches_the_dispatcher() {
        let mut app = suppression_app();
        spawn_vertical_separator(&mut app, SplitId(1), 320.0, 400.0);

        press_at(&mut app, Vec2::new(600.0, 200.0));

        assert!(pressed_a_pane(&app));
    }
}
