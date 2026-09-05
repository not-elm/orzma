//! The latest layout snapshot the drain received, and the system that
//! applies it to pane nodes.

use crate::registry::PaneRegistry;
use crate::{MuxPane, MuxSystems};
use bevy::prelude::*;
use orzma_mux::prelude::{Layout, PaneId, PaneRect, Separator, SplitOrientation};
use orzma_tty::CellPixels;

/// The latest layout snapshot. Written by the drain only when it
/// differs; the non-empty → empty transition is detected there.
#[derive(Resource, Default, Debug, PartialEq)]
pub struct CurrentLayout(pub Layout);

/// What the host needs to turn cells into logical px.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PaneGeometry {
    /// Physical pixels per cell.
    pub cell_px: CellPixels,
    /// The window's scale factor (physical / logical).
    pub scale_factor: f32,
}

/// A separator node between two panes.
#[derive(Component, Debug)]
pub struct MuxSeparator;

/// The GUI accepted a new active pane from a `Layout`. `previous` is
/// the last accepted active's entity; it may already be despawned (a
/// `PaneClosed` in the same drain), in which case it resolves to
/// `None`.
#[derive(Event, Debug, Clone, Copy)]
pub struct MuxActivePaneChanged {
    /// The entity that was the applied active before this change.
    pub previous: Option<Entity>,
    /// The entity that is the applied active after this change.
    pub current: Option<Entity>,
}

/// The absolute node for `rect`: physical px are integral
/// (`cells × cell_px`), divided by the scale factor for `Val::Px`.
pub fn pane_node(rect: &PaneRect, geometry: &PaneGeometry) -> Node {
    let px = |cells: u16, pitch: u16| f32::from(cells) * f32::from(pitch) / geometry.scale_factor;
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(px(rect.x, geometry.cell_px.width)),
        top: Val::Px(px(rect.y, geometry.cell_px.height)),
        width: Val::Px(px(rect.cols, geometry.cell_px.width)),
        height: Val::Px(px(rect.rows, geometry.cell_px.height)),
        ..default()
    }
}

/// Registers `apply_layout`, gated on a changed layout or geometry.
pub(crate) struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            apply_layout.in_set(MuxSystems::ApplyLayout).run_if(
                resource_exists::<PaneGeometry>.and_then(
                    resource_exists_and_changed::<CurrentLayout>
                        .or_else(resource_exists_and_changed::<PaneGeometry>),
                ),
            ),
        );
    }
}

/// Separator colour until it is configurable.
const SEPARATOR_COLOR: Color = Color::srgb(0.35, 0.35, 0.40);

/// Runs when `CurrentLayout` or `PaneGeometry` changed (see the plugin).
fn apply_layout(
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    mut nodes: Query<&mut Node, With<MuxPane>>,
    mut separators: Query<
        (Entity, &mut Node, Option<&ChildOf>),
        (With<MuxSeparator>, Without<MuxPane>),
    >,
    current: Res<CurrentLayout>,
    geometry: Res<PaneGeometry>,
    parents: Query<&ChildOf, With<MuxPane>>,
) {
    let layout = &current.0;
    for rect in &layout.panes {
        let Some(entity) = registry.entity_of(rect.pane) else {
            continue;
        };
        if let Ok(mut node) = nodes.get_mut(entity) {
            apply_pane_node(&mut node, &pane_node(rect, &geometry));
        }
    }
    let container = container_of(&registry, layout, &parents);
    reconcile_separators(&mut commands, &mut separators, layout, &geometry, container);
    apply_active(&mut commands, &mut registry, layout);
}

/// Writes `wanted`'s absolute geometry into `node` only when it
/// differs, so an unchanged pane produces no `Node` mutation.
///
/// Takes `Mut<'_, Node>` rather than `&mut Node`: coercing a query
/// item's `Mut<Node>` to a plain `&mut Node` at the call site would
/// already call `DerefMut::deref_mut` (which marks the component
/// changed) before this comparison ever ran. Reading fields through
/// `node` here goes through `Mut`'s immutable `Deref` instead, so only
/// the assignments inside the branch below — reached exclusively when
/// a field actually differs — mark the component changed.
fn apply_pane_node(node: &mut Mut<'_, Node>, wanted: &Node) {
    if node.position_type == wanted.position_type
        && node.left == wanted.left
        && node.top == wanted.top
        && node.width == wanted.width
        && node.height == wanted.height
    {
        return;
    }
    node.position_type = wanted.position_type;
    node.left = wanted.left;
    node.top = wanted.top;
    node.width = wanted.width;
    node.height = wanted.height;
}

/// The container the pane entities live under, so separators become
/// its children too; `None` before the first pane is parented.
fn container_of(
    registry: &PaneRegistry,
    layout: &Layout,
    parents: &Query<&ChildOf, With<MuxPane>>,
) -> Option<Entity> {
    layout
        .panes
        .iter()
        .filter_map(|rect| registry.entity_of(rect.pane))
        .find_map(|entity| parents.get(entity).ok().map(ChildOf::parent))
}

/// Spawns, updates, or despawns separator nodes to match the layout. An
/// existing separator with no `ChildOf` yet (spawned before `container`
/// was known) is re-parented once `container` resolves.
fn reconcile_separators(
    commands: &mut Commands,
    separators: &mut Query<
        (Entity, &mut Node, Option<&ChildOf>),
        (With<MuxSeparator>, Without<MuxPane>),
    >,
    layout: &Layout,
    geometry: &PaneGeometry,
    container: Option<Entity>,
) {
    let mut existing: Vec<Entity> = separators.iter().map(|(entity, ..)| entity).collect();
    existing.sort();
    for (index, separator) in layout.separators.iter().enumerate() {
        let wanted = separator_node(separator, geometry);
        match existing.get(index) {
            Some(entity) => {
                if let Ok((_, mut node, child_of)) = separators.get_mut(*entity) {
                    apply_pane_node(&mut node, &wanted);
                    if child_of.is_none()
                        && let Some(container) = container
                    {
                        commands.entity(*entity).try_insert(ChildOf(container));
                    }
                }
            }
            None => {
                let mut spawned =
                    commands.spawn((MuxSeparator, wanted, BackgroundColor(SEPARATOR_COLOR)));
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

/// The absolute node for one separator, expressed as a one-cell-thick
/// `PaneRect` along its orientation.
fn separator_node(separator: &Separator, geometry: &PaneGeometry) -> Node {
    let rect = PaneRect {
        pane: PaneId(0),
        x: separator.x,
        y: separator.y,
        cols: match separator.orientation {
            SplitOrientation::Vertical => 1,
            SplitOrientation::Horizontal => separator.len,
        },
        rows: match separator.orientation {
            SplitOrientation::Vertical => separator.len,
            SplitOrientation::Horizontal => 1,
        },
    };
    pane_node(&rect, geometry)
}

/// Accepts `layout.active` unless it predates the GUI's last
/// `SelectPane`, and reports a change against the last accepted active.
fn apply_active(commands: &mut Commands, registry: &mut PaneRegistry, layout: &Layout) {
    if registry.last_select.is_some_and(|sent| layout.seq < sent) {
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
    commands.trigger(MuxActivePaneChanged { previous, current });
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_mux::prelude::{CommandSeq, Layout, PaneId, PaneRect, Separator, SplitOrientation};
    use orzma_tty::CellPixels;
    use orzma_vt::prelude::GridSize;

    #[derive(Resource, Default)]
    struct Changes(Vec<(Option<Entity>, Option<Entity>)>);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(LayoutPlugin)
            .init_resource::<PaneRegistry>()
            .init_resource::<CurrentLayout>()
            .init_resource::<Changes>()
            .insert_resource(PaneGeometry {
                cell_px: CellPixels {
                    width: 10,
                    height: 20,
                },
                scale_factor: 2.0,
            })
            .add_observer(
                |ev: On<MuxActivePaneChanged>, mut changes: ResMut<Changes>| {
                    changes.0.push((ev.previous, ev.current))
                },
            );
        app
    }

    fn two_panes(app: &mut App) -> (Entity, Entity) {
        let a = app
            .world_mut()
            .spawn((MuxPane(PaneId(1)), Node::default()))
            .id();
        let b = app
            .world_mut()
            .spawn((MuxPane(PaneId(2)), Node::default()))
            .id();
        let mut registry = app.world_mut().resource_mut::<PaneRegistry>();
        registry.panes.insert(PaneId(1), a);
        registry.panes.insert(PaneId(2), b);
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
    /// physical px, and that one separator node exists per separator.
    ///
    /// Case: two panes side by side at a 10×20 px cell on a 2× display.
    #[test]
    fn apply_layout_positions_panes_and_separators_in_logical_px() {
        let mut app = app();
        let (a, b) = two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();
        let node_a = app.world().get::<Node>(a).unwrap();
        assert_eq!((node_a.left, node_a.width), (Val::Px(0.0), Val::Px(200.0)));
        let node_b = app.world().get::<Node>(b).unwrap();
        assert_eq!(
            (node_b.left, node_b.top, node_b.width, node_b.height),
            (Val::Px(205.0), Val::Px(0.0), Val::Px(200.0), Val::Px(240.0))
        );
        let world = app.world_mut();
        let seps: Vec<&Node> = world
            .query_filtered::<&Node, With<MuxSeparator>>()
            .iter(world)
            .collect();
        assert_eq!(seps.len(), 1);
        assert_eq!(
            (seps[0].left, seps[0].width, seps[0].height),
            (Val::Px(200.0), Val::Px(5.0), Val::Px(240.0))
        );
    }

    /// Asserts that an accepted active change fires
    /// `MuxActivePaneChanged` with the previously applied active, and
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
        assert_eq!(app.world().get::<Node>(a).unwrap().width, Val::Px(400.0));
    }

    /// Asserts that a separator spawned before any pane was parented is
    /// re-parented under the container once one becomes resolvable,
    /// rather than staying parentless for its whole life.
    ///
    /// Case: the first `Layout` arrives before the shell surface has
    /// parented the pane entities, so `container_of` first resolves to
    /// `None`; a later frame parents the panes and reapplies the layout.
    #[test]
    fn an_unparented_separator_is_reparented_once_a_container_resolves() {
        let mut app = app();
        let (a, b) = two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();
        let separator = app
            .world_mut()
            .query_filtered::<Entity, With<MuxSeparator>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<ChildOf>(separator).is_none());

        let container = app.world_mut().spawn(Node::default()).id();
        app.world_mut().entity_mut(a).insert(ChildOf(container));
        app.world_mut().entity_mut(b).insert(ChildOf(container));
        app.world_mut().insert_resource(PaneGeometry {
            cell_px: CellPixels {
                width: 20,
                height: 20,
            },
            scale_factor: 2.0,
        });
        app.update();

        assert_eq!(
            app.world().get::<ChildOf>(separator).map(ChildOf::parent),
            Some(container)
        );
    }

    /// Asserts that reapplying a layout whose pane rectangles are
    /// unchanged leaves every pane `Node` unflagged, so a system gated
    /// on `Changed<Node>` does not re-run on a no-op layout update.
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
              changed_nodes: Query<(), (Changed<Node>, With<MuxPane>)>| {
                changed.0 = changed_nodes.iter().count();
            })
            .after(MuxSystems::ApplyLayout),
        );
        two_panes(&mut app);
        set_layout(&mut app, 1, PaneId(1));
        app.update();

        set_layout(&mut app, 1, PaneId(1));
        app.update();
        assert_eq!(app.world().resource::<ChangedPaneNodes>().0, 0);
    }
}
