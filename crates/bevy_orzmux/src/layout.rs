//! The latest layout snapshot the drain received, applied to the pane
//! and separator nodes.

use crate::registry::PaneRegistry;
use crate::{OrzmuxPane, OrzmuxSystems};
use bevy::prelude::*;
use orzma_tty::CellPixels;
use orzmux::prelude::{Layout, PaneRect, Separator, SplitOrientation};

/// The latest layout snapshot, marked changed only when it differs.
#[derive(Resource, Default, Debug, PartialEq)]
pub(crate) struct CurrentLayout(pub Layout);

/// What the host needs to turn cells into logical px.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PaneGeometry {
    /// Physical pixels per cell.
    pub cell_px: CellPixels,
    /// The window's scale factor (physical / logical).
    pub scale_factor: f32,
}

/// The node every pane entity is parented under; separators are spawned
/// as its children too. The host marks its clipping container with it
/// before requesting the first pane.
#[derive(Component, Debug)]
pub struct OrzmuxPaneContainer;

/// A separator node between two panes.
#[derive(Component, Debug)]
pub(crate) struct OrzmuxSeparator;

/// The GUI accepted a new active pane from a `Layout`. `previous`
/// resolves to `None` when that entity was already despawned by a
/// `PaneClosed` in the same drain.
#[derive(Event, Debug, Clone, Copy)]
pub struct OrzmuxActivePaneChanged {
    /// The entity that was the applied active before this change.
    pub previous: Option<Entity>,
    /// The entity that is the applied active after this change.
    pub current: Option<Entity>,
}

/// An absolutely positioned node with the given logical-px edges.
pub fn absolute_px_node(left: f32, top: f32, width: f32, height: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(left),
        top: Val::Px(top),
        width: Val::Px(width),
        height: Val::Px(height),
        ..default()
    }
}

/// Positions pane nodes and separators from the backend's latest
/// layout.
pub(crate) struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            apply_layout.in_set(OrzmuxSystems::ApplyLayout).run_if(
                resource_exists::<PaneGeometry>.and_then(
                    resource_exists_and_changed::<CurrentLayout>
                        .or_else(resource_exists_and_changed::<PaneGeometry>),
                ),
            ),
        );
    }
}

/// Separator colour.
///
/// TODO: make the colour configurable.
const SEPARATOR_COLOR: Color = Color::srgb(0.35, 0.35, 0.40);

/// Logical-px thickness of the line painted inside a reserved separator
/// cell, before rounding to whole physical px (never below one).
const SEPARATOR_THICKNESS_LOGICAL_PX: f32 = 1.0;

fn apply_layout(
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    mut nodes: Query<&mut Node, With<OrzmuxPane>>,
    mut separators: Query<(Entity, &mut Node), (With<OrzmuxSeparator>, Without<OrzmuxPane>)>,
    current: Res<CurrentLayout>,
    geometry: Res<PaneGeometry>,
    container: Query<Entity, With<OrzmuxPaneContainer>>,
) {
    let layout = &current.0;
    for rect in &layout.panes {
        let Some(entity) = registry.entity_of(rect.pane) else {
            continue;
        };
        if let Ok(mut node) = nodes.get_mut(entity) {
            node.set_if_neq(pane_node(rect, layout, &geometry));
        }
    }
    let container = container.single().ok();
    reconcile_separators(&mut commands, &mut separators, layout, &geometry, container);
    apply_active(&mut commands, &mut registry, layout);
}

/// Spawns, updates, or despawns separator nodes to match the layout,
/// parenting new ones under `container` when the host has marked one.
fn reconcile_separators(
    commands: &mut Commands,
    separators: &mut Query<(Entity, &mut Node), (With<OrzmuxSeparator>, Without<OrzmuxPane>)>,
    layout: &Layout,
    geometry: &PaneGeometry,
    container: Option<Entity>,
) {
    let mut existing: Vec<Entity> = separators.iter().map(|(entity, _)| entity).collect();
    existing.sort();
    for (index, separator) in layout.separators.iter().enumerate() {
        let wanted = separator_node(separator, layout, geometry);
        match existing.get(index) {
            Some(entity) => {
                if let Ok((_, mut node)) = separators.get_mut(*entity) {
                    node.set_if_neq(wanted);
                }
            }
            None => {
                let mut spawned =
                    commands.spawn((OrzmuxSeparator, wanted, BackgroundColor(SEPARATOR_COLOR)));
                if let Some(container) = container {
                    spawned.insert(ChildOf(container));
                }
            }
        }
    }
    for entity in existing.iter().skip(layout.separators.len()) {
        commands.entity(*entity).despawn();
    }
}

/// The absolute node for `rect`: physical px are integral
/// (`cells × cell_px`), divided by the scale factor for `Val::Px`.
///
/// A right or bottom edge that stops short of the layout size grows into
/// the reserved separator cell by everything but the line, so only the
/// line's thickness separates two panes on either axis.
fn pane_node(rect: &PaneRect, layout: &Layout, geometry: &PaneGeometry) -> Node {
    let scale = geometry.scale_factor;
    let (cell_w, cell_h) = cell_pitch_phys(geometry);
    let thickness = line_thickness_phys(geometry);
    let bleed_x = gap_before_line(rect.x, rect.cols, layout.size.cols, cell_w, thickness);
    let bleed_y = gap_before_line(rect.y, rect.rows, layout.size.rows, cell_h, thickness);
    absolute_px_node(
        f32::from(rect.x) * cell_w / scale,
        f32::from(rect.y) * cell_h / scale,
        (f32::from(rect.cols) * cell_w + bleed_x) / scale,
        (f32::from(rect.rows) * cell_h + bleed_y) / scale,
    )
}

/// The node for a separator: a line `SEPARATOR_THICKNESS_LOGICAL_PX`
/// thick occupying the far end of the one cell the layout reserves for
/// it, flush against the pane that follows, spanning the separator's
/// full length.
///
/// A separator that stops short of the layout size ends inside the cell
/// reserved for a crossing line, so its far end is extended across that
/// cell to meet the crossing line. All offsets are whole physical px, so
/// the line never rounds away to nothing.
fn separator_node(separator: &Separator, layout: &Layout, geometry: &PaneGeometry) -> Node {
    let scale = geometry.scale_factor;
    let (cell_w, cell_h) = cell_pitch_phys(geometry);
    let thickness = line_thickness_phys(geometry);
    let x = f32::from(separator.x);
    let y = f32::from(separator.y);
    let len = f32::from(separator.len);
    let (left, top, width, height) = match separator.orientation {
        SplitOrientation::Vertical => (
            x * cell_w + (cell_w - thickness).max(0.0),
            y * cell_h,
            thickness,
            len * cell_h
                + gap_before_line(
                    separator.y,
                    separator.len,
                    layout.size.rows,
                    cell_h,
                    thickness,
                ),
        ),
        SplitOrientation::Horizontal => (
            x * cell_w,
            y * cell_h + (cell_h - thickness).max(0.0),
            len * cell_w
                + gap_before_line(
                    separator.x,
                    separator.len,
                    layout.size.cols,
                    cell_w,
                    thickness,
                ),
            thickness,
        ),
    };
    absolute_px_node(left / scale, top / scale, width / scale, height / scale)
}

/// The physical px between an extent's far edge and the separator line
/// that follows it: the reserved cell minus the line when the extent
/// stops short of `limit`, zero when it reaches the layout edge. The
/// split tree tiles the layout size, so every edge short of it is
/// followed by exactly one separator cell.
fn gap_before_line(start: u16, extent: u16, limit: u16, cell: f32, thickness: f32) -> f32 {
    if start + extent < limit {
        (cell - thickness).max(0.0)
    } else {
        0.0
    }
}

/// The cell pitch as `(width, height)` in physical px.
fn cell_pitch_phys(geometry: &PaneGeometry) -> (f32, f32) {
    (
        f32::from(geometry.cell_px.width),
        f32::from(geometry.cell_px.height),
    )
}

/// The separator line's thickness in whole physical px, never below one.
fn line_thickness_phys(geometry: &PaneGeometry) -> f32 {
    (SEPARATOR_THICKNESS_LOGICAL_PX * geometry.scale_factor)
        .round()
        .max(1.0)
}

/// Accepts `layout.active` unless it predates the GUI's last
/// `SelectPane` while the applied active pane is still open, and
/// reports a change against the applied active. A stale layout is
/// accepted once the applied pane is gone.
fn apply_active(commands: &mut Commands, registry: &mut PaneRegistry, layout: &Layout) {
    let stale = registry.last_select.is_some_and(|sent| layout.seq < sent);
    let applied_open = registry
        .applied_active
        .is_none_or(|active| registry.entity_of(active).is_some());
    if stale && applied_open {
        return;
    }
    if registry.applied_active == layout.active {
        return;
    }
    let previous = registry
        .applied_active
        .and_then(|pane| registry.entity_of(pane));
    let current = layout.active.and_then(|pane| registry.entity_of(pane));
    registry.applied_active = layout.active;
    commands.trigger(OrzmuxActivePaneChanged { previous, current });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, spawn_pane};
    use orzma_tty::CellPixels;
    use orzma_vt::prelude::GridSize;
    use orzmux::prelude::{CommandSeq, Layout, PaneId, PaneRect, Separator, SplitOrientation};

    #[derive(Resource, Default)]
    struct Changes(Vec<(Option<Entity>, Option<Entity>)>);

    /// An app with the layout applier, a 10×20 px cell on a 2× display,
    /// and a marked pane container.
    fn app() -> App {
        let (mut app, _commands) = app_with_connection(LayoutPlugin);
        app.init_resource::<Changes>()
            .insert_resource(PaneGeometry {
                cell_px: CellPixels {
                    width: 10,
                    height: 20,
                },
                scale_factor: 2.0,
            })
            .add_observer(
                |ev: On<OrzmuxActivePaneChanged>, mut changes: ResMut<Changes>| {
                    changes.0.push((ev.previous, ev.current))
                },
            );
        app.world_mut()
            .spawn((OrzmuxPaneContainer, Node::default()));
        app
    }

    fn two_panes(app: &mut App) -> (Entity, Entity) {
        let a = spawn_pane(app, PaneId(1));
        let b = spawn_pane(app, PaneId(2));
        for entity in [a, b] {
            app.world_mut().entity_mut(entity).insert(Node::default());
        }
        (a, b)
    }

    fn set_layout(app: &mut App, seq: u64, active: PaneId) {
        app.world_mut().resource_mut::<CurrentLayout>().0 = Layout {
            seq: CommandSeq(seq),
            size: GridSize { cols: 81, rows: 24 },
            active: Some(active),
            panes: vec![
                PaneRect {
                    pane: PaneId(1),
                    x: 0,
                    y: 0,
                    cols: 40,
                    rows: 24,
                },
                PaneRect {
                    pane: PaneId(2),
                    x: 41,
                    y: 0,
                    cols: 40,
                    rows: 24,
                },
            ],
            separators: vec![Separator {
                orientation: SplitOrientation::Vertical,
                x: 40,
                y: 0,
                len: 24,
            }],
        };
    }

    /// Asserts that pane nodes are positioned in logical px from integral
    /// physical px, that the pane before a separator grows into the
    /// reserved cell up to the line, that the line sits flush against
    /// the pane after it, and that the separator is a child of the
    /// marked container.
    ///
    /// Case: two panes side by side at a 10×20 px cell on a 2× display.
    #[test]
    fn apply_layout_positions_panes_and_separators_in_logical_px() {
        let mut app = app();
        let (a, b) = two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();
        let container = app
            .world_mut()
            .query_filtered::<Entity, With<OrzmuxPaneContainer>>()
            .single(app.world())
            .unwrap();
        let separator = app
            .world_mut()
            .query_filtered::<Entity, With<OrzmuxSeparator>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            app.world().get::<ChildOf>(separator).map(ChildOf::parent),
            Some(container)
        );
        let node_a = app.world().get::<Node>(a).unwrap();
        assert_eq!((node_a.left, node_a.width), (Val::Px(0.0), Val::Px(204.0)));
        let node_b = app.world().get::<Node>(b).unwrap();
        assert_eq!(
            (node_b.left, node_b.top, node_b.width, node_b.height),
            (Val::Px(205.0), Val::Px(0.0), Val::Px(200.0), Val::Px(240.0))
        );
        let world = app.world_mut();
        let seps: Vec<&Node> = world
            .query_filtered::<&Node, With<OrzmuxSeparator>>()
            .iter(world)
            .collect();
        assert_eq!(seps.len(), 1);
        assert_eq!(
            (seps[0].left, seps[0].top, seps[0].width, seps[0].height),
            (Val::Px(204.0), Val::Px(0.0), Val::Px(1.0), Val::Px(240.0))
        );
    }

    /// Asserts that a separator line occupies the last one logical px,
    /// rounded to whole physical px, of its reserved cell on the axis it
    /// divides, for both orientations and on a 1× display.
    ///
    /// Case: a stacked split on a 2× display and a side-by-side split on
    /// a 1× display, each with a different cell pitch.
    #[test]
    fn separator_lines_sit_flush_against_the_following_pane() {
        let full_width = Layout {
            size: GridSize { cols: 81, rows: 24 },
            ..Layout::default()
        };
        let hidpi = PaneGeometry {
            cell_px: CellPixels {
                width: 10,
                height: 20,
            },
            scale_factor: 2.0,
        };
        let horizontal = separator_node(
            &Separator {
                orientation: SplitOrientation::Horizontal,
                x: 0,
                y: 12,
                len: 81,
            },
            &full_width,
            &hidpi,
        );
        assert_eq!(
            (
                horizontal.left,
                horizontal.top,
                horizontal.width,
                horizontal.height
            ),
            (Val::Px(0.0), Val::Px(129.0), Val::Px(405.0), Val::Px(1.0))
        );

        let lodpi = PaneGeometry {
            cell_px: CellPixels {
                width: 8,
                height: 16,
            },
            scale_factor: 1.0,
        };
        let vertical = separator_node(
            &Separator {
                orientation: SplitOrientation::Vertical,
                x: 3,
                y: 0,
                len: 24,
            },
            &full_width,
            &lodpi,
        );
        assert_eq!(
            (vertical.left, vertical.top, vertical.width, vertical.height),
            (Val::Px(31.0), Val::Px(0.0), Val::Px(1.0), Val::Px(384.0))
        );
    }

    fn stacked_then_side_by_side() -> Layout {
        Layout {
            seq: CommandSeq(1),
            size: GridSize { cols: 81, rows: 24 },
            active: Some(PaneId(1)),
            panes: vec![
                PaneRect {
                    pane: PaneId(1),
                    x: 0,
                    y: 0,
                    cols: 40,
                    rows: 12,
                },
                PaneRect {
                    pane: PaneId(2),
                    x: 0,
                    y: 13,
                    cols: 40,
                    rows: 11,
                },
                PaneRect {
                    pane: PaneId(3),
                    x: 41,
                    y: 0,
                    cols: 40,
                    rows: 24,
                },
            ],
            separators: vec![
                Separator {
                    orientation: SplitOrientation::Horizontal,
                    x: 0,
                    y: 12,
                    len: 40,
                },
                Separator {
                    orientation: SplitOrientation::Vertical,
                    x: 40,
                    y: 0,
                    len: 24,
                },
            ],
        }
    }

    /// Asserts that a pane grows by the reserved cell minus the line on
    /// each edge that meets a separator, and not at all on edges that
    /// reach the window or sit after a line.
    ///
    /// Case: the left half is split top and bottom, so the top-left pane
    /// meets a separator on its right and bottom, the bottom-left pane
    /// only on its right, and the right pane on neither.
    #[test]
    fn panes_bleed_into_the_reserved_cell_only_on_edges_that_meet_a_separator() {
        let layout = stacked_then_side_by_side();
        let geometry = PaneGeometry {
            cell_px: CellPixels {
                width: 10,
                height: 20,
            },
            scale_factor: 2.0,
        };
        let size = |node: Node| (node.left, node.top, node.width, node.height);
        assert_eq!(
            size(pane_node(&layout.panes[0], &layout, &geometry)),
            (Val::Px(0.0), Val::Px(0.0), Val::Px(204.0), Val::Px(129.0))
        );
        assert_eq!(
            size(pane_node(&layout.panes[1], &layout, &geometry)),
            (Val::Px(0.0), Val::Px(130.0), Val::Px(204.0), Val::Px(110.0))
        );
        assert_eq!(
            size(pane_node(&layout.panes[2], &layout, &geometry)),
            (Val::Px(205.0), Val::Px(0.0), Val::Px(200.0), Val::Px(240.0))
        );
    }

    /// Asserts that a separator which stops short of the window edge is
    /// extended across the reserved cell it ends in, so it meets the
    /// crossing line instead of leaving a gap at the junction.
    ///
    /// Case: the horizontal divider of a stacked left half ends at the
    /// column reserved for the vertical divider.
    #[test]
    fn a_separator_ending_at_a_crossing_line_extends_to_meet_it() {
        let layout = stacked_then_side_by_side();
        let geometry = PaneGeometry {
            cell_px: CellPixels {
                width: 10,
                height: 20,
            },
            scale_factor: 2.0,
        };
        let horizontal = separator_node(&layout.separators[0], &layout, &geometry);
        assert_eq!(
            (horizontal.left, horizontal.width),
            (Val::Px(0.0), Val::Px(204.0))
        );
        let vertical = separator_node(&layout.separators[1], &layout, &geometry);
        assert_eq!(
            (vertical.top, vertical.height),
            (Val::Px(0.0), Val::Px(240.0))
        );
    }

    /// Asserts that an accepted active change fires
    /// `OrzmuxActivePaneChanged` with the previously applied active, and
    /// that a stale layout leaves the applied active untouched.
    ///
    /// Case: the user presses select-right (seq 10) then clicks the left
    /// pane (seq 11); the seq-10 layout arrives after the click.
    #[test]
    fn active_changes_follow_the_applied_active_and_ignore_stale_layouts() {
        let mut app = app();
        let (a, b) = two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();
        assert_eq!(app.world().resource::<Changes>().0, vec![(None, Some(a))]);

        app.world_mut().resource_mut::<PaneRegistry>().last_select = Some(CommandSeq(11));
        set_layout(&mut app, 10, PaneId(2));
        app.update();
        assert_eq!(
            app.world().resource::<Changes>().0.len(),
            1,
            "a stale layout does not move focus"
        );
        assert_eq!(
            app.world().resource::<PaneRegistry>().applied_active,
            Some(PaneId(1))
        );

        set_layout(&mut app, 11, PaneId(2));
        app.update();
        assert_eq!(
            app.world().resource::<Changes>().0.last(),
            Some(&(Some(a), Some(b)))
        );
    }

    /// Asserts that a layout predating the last `SelectPane` is still
    /// accepted once the applied active pane is gone, reporting the
    /// change with no previous entity.
    ///
    /// Case: the user clicks pane two and its shell exits before the
    /// backend handles the click, so the exit's layout naming pane one
    /// arrives with an older sequence than the click's.
    #[test]
    fn a_stale_layout_is_accepted_once_the_applied_pane_is_gone() {
        let mut app = app();
        let (a, b) = two_panes(&mut app);
        {
            let mut registry = app.world_mut().resource_mut::<PaneRegistry>();
            registry.applied_active = Some(PaneId(2));
            registry.last_select = Some(CommandSeq(10));
            registry.panes.remove(&PaneId(2));
        }
        app.world_mut().entity_mut(b).despawn();
        set_layout(&mut app, 9, PaneId(1));
        app.update();
        assert_eq!(app.world().resource::<Changes>().0, vec![(None, Some(a))]);
        assert_eq!(
            app.world().resource::<PaneRegistry>().applied_active,
            Some(PaneId(1))
        );
    }

    /// Asserts that a pixel-pitch change alone re-lays out the panes.
    ///
    /// Case: the user changes the font size with two panes open.
    #[test]
    fn a_geometry_change_alone_reapplies_the_layout() {
        let mut app = app();
        let (a, _b) = two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();
        app.world_mut().insert_resource(PaneGeometry {
            cell_px: CellPixels {
                width: 20,
                height: 20,
            },
            scale_factor: 2.0,
        });
        app.update();
        assert_eq!(app.world().get::<Node>(a).unwrap().width, Val::Px(409.0));
    }

    /// Asserts that reapplying a layout whose pane rectangles are
    /// unchanged leaves every pane `Node` unflagged.
    ///
    /// Case: the backend resends the same layout as part of an
    /// unrelated event batch, such as a `Layout` carrying only a fresh
    /// bootstrap `Frame` for an already-placed pane.
    #[test]
    fn reapplying_an_unchanged_layout_does_not_mark_pane_nodes_changed() {
        #[derive(Resource, Default)]
        struct ChangedPaneNodes(usize);

        let mut app = app();
        app.init_resource::<ChangedPaneNodes>().add_systems(
            Update,
            (|mut changed: ResMut<ChangedPaneNodes>,
              changed_nodes: Query<(), (Changed<Node>, With<OrzmuxPane>)>| {
                changed.0 = changed_nodes.iter().count();
            })
            .after(OrzmuxSystems::ApplyLayout),
        );
        two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();

        set_layout(&mut app, 1, PaneId(1));
        app.update();
        assert_eq!(app.world().resource::<ChangedPaneNodes>().0, 0);
    }
}
