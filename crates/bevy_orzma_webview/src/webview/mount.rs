//! Webview mounting: `ChildOf` children of a terminal surface that the
//! webview host mounts, resizes, and unmounts, kept in step with the cell
//! metrics and projected into the terminal's `TerminalOverlays`.

use super::forward_keys::ForwardKeys;
use super::render::preload::build_preload;
use crate::error::{WebviewError, WebviewResult};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::{Render, RenderApp, render_asset::prepare_assets};
use bevy::ui_render::PreparedUiMaterial;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::{
    FocusedWebview, PreloadScripts, WebviewGpuImageInjectSet, WebviewSize, WebviewSource,
    WebviewTextureTarget,
};
use bevy_orzma_tty_renderer::prelude::{
    OVERLAY_SLOTS, TerminalCellMetricsResource, TerminalMaterialSystems, TerminalOverlays,
    TerminalUiMaterial, TerminalView,
};
use bevy_orzmux::prelude::{OrzmuxConnection, OrzmuxWebviewEvent};
use orzma_vt::prelude::{InstanceId, PlacementSize};
use orzma_webview_host::prelude::{HandleId, MountId, MountSpec, WebviewCommand, WebviewEvent};
use orzmux::prelude::OrzmuxCommand;

/// Marks a webview as render-only: no pointer or keyboard input reaches the
/// embedded page.
#[derive(Component, Debug, Default)]
pub struct NonInteractive;

/// Marks a webview entity and records its identity: the mount the host
/// minted for it, the placement and registration it shows, the overlay
/// texture slot it occupies on its parent terminal, and the rectangle it
/// reserved. The owning terminal surface is the `ChildOf` parent.
///
/// # Invariants
///
/// `AnchoredPlacement.size` for this instance always equals `rows` and
/// `cols` here.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Webview {
    handle: HandleId,
    instance: InstanceId,
    mount: MountId,
    slot: u8,
    rows: u16,
    cols: u16,
}

/// A pointer hit on an interactive webview rect: the child entity that
/// owns the rect and the pointer position in webview-local DIP (logical px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebviewHit {
    /// The interactive webview child under the pointer.
    pub child: Entity,
    /// `(local_phys − rect_origin_phys) / scale_factor` — the pointer in
    /// webview-local DIP, the coordinate space CEF mouse events expect.
    pub local_dip: Vec2,
}

/// Marks a webview whose first successful projection into `TerminalOverlays`
/// has been reported to the host as `Composited`, so later projections of
/// the same rect report nothing.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositeNotified;

/// Registers the webview runtime: the observer that spawns, resizes, and
/// despawns webviews on the host's mount events, the `WebviewSize` size sync,
/// the per-frame projection that derives `TerminalOverlays` from the
/// frame-carried placement list, and the render-world ordering edge that
/// keeps webview GPU texture injection ahead of the terminal material's
/// bind-group rebuild.
pub(crate) struct WebviewPlugin;

impl Webview {
    /// The webview of `mount`, showing `handle`'s content for `instance` in
    /// overlay texture `slot` (0..`OVERLAY_SLOTS`) of its terminal, over a
    /// rect of `rows` × `cols` cells.
    pub fn new(
        handle: HandleId,
        instance: InstanceId,
        mount: MountId,
        slot: u8,
        rows: u16,
        cols: u16,
    ) -> Self {
        Self {
            handle,
            instance,
            mount,
            slot,
            rows,
            cols,
        }
    }

    /// The registration this webview's content comes from.
    pub fn handle(&self) -> &HandleId {
        &self.handle
    }

    /// The placement this webview shows; frames address the placement by it.
    pub fn instance(&self) -> InstanceId {
        self.instance
    }

    /// The mount the host minted for this webview.
    pub fn mount(&self) -> MountId {
        self.mount
    }

    /// The overlay texture slot on the parent terminal.
    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// Rect height in terminal cells.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Rect width in terminal cells.
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// This webview with its rect resized to `size`.
    pub fn resized(&self, size: PlacementSize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            ..self.clone()
        }
    }
}

impl Plugin for WebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_webview_size)
            .add_systems(
                PostUpdate,
                project_webview_overlays.before(TerminalMaterialSystems::UpdateMaterial),
            )
            .add_observer(apply_mount_event);
        // NOTE: without this edge, `TerminalUiMaterial`'s bind-group rebuild
        // can run between bevy_cef's rebind image-touch (which re-uploads the
        // CPU placeholder) and the GPU texture injection, capturing the
        // placeholder permanently — a forever-black overlay (see the
        // `WebviewGpuImageInjectSet` docs in bevy_cef's texture_target.rs).
        // It also transitively orders the GpuImage prepare before the
        // material prepare, avoiding a 1-frame RetryNextUpdate stall.
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.configure_sets(
                Render,
                WebviewGpuImageInjectSet
                    .before(prepare_assets::<PreparedUiMaterial<TerminalUiMaterial>>),
            );
        }
    }
}

/// Returns the webview entity that currently holds keyboard focus on
/// `active_surface`: `Some(e)` iff `FocusedWebview` points at `e`, `e` carries
/// `Webview`, and its `ChildOf` parent is the active surface.
pub fn focused_webview_of(
    focused: Option<&FocusedWebview>,
    webview_parents: &Query<&ChildOf, With<Webview>>,
    active_surface: Option<Entity>,
) -> Option<Entity> {
    let candidate = focused?.0?;
    let parent = webview_parents.get(candidate).ok()?.parent();
    (Some(parent) == active_surface).then_some(candidate)
}

/// Hit-tests a terminal-local physical-pixel point against the terminal's
/// ACTIVE inline overlay rects and returns the interactive child whose rect
/// contains it.
///
/// Cell coordinates are 0-indexed (`row = floor(local_phys.y / cell_h)`, and
/// the column analog). `rows == 0` sentinel slots never match, and a
/// partially-scrolled rect with a negative `row` origin still hits in its
/// visible cells. `NonInteractive` children are invisible to the hit-test.
pub fn webview_hit_at(
    children: &Query<&Children>,
    webviews: &Query<(&Webview, Has<NonInteractive>)>,
    overlays: &TerminalOverlays,
    terminal: Entity,
    local_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Option<WebviewHit> {
    let row = (local_phys.y / cell_h_phys).floor() as i32;
    let col = (local_phys.x / cell_w_phys).floor() as i32;
    let kids = children.get(terminal).ok()?;
    kids.iter().find_map(|child| {
        let Ok((view, non_interactive)) = webviews.get(child) else {
            return None;
        };
        if non_interactive {
            return None;
        }
        let rect = *overlays.rects.get(usize::from(view.slot))?;
        let contains = rect.z != 0
            && row >= rect.x
            && row < rect.x + rect.z
            && col >= rect.y
            && col < rect.y + rect.w;
        if !contains {
            return None;
        }
        let local_dip = webview_local_dip(
            overlays,
            view.slot,
            local_phys,
            cell_w_phys,
            cell_h_phys,
            scale_factor,
        )?;
        Some(WebviewHit { child, local_dip })
    })
}

/// Converts a terminal-local physical-pixel point to webview-local DIP
/// relative to a slot's active overlay rect, WITHOUT containment checking, so
/// a point that lies off the rect still produces a (possibly out-of-view)
/// position. Returns `None` for an out-of-range slot or a `rows == 0`
/// sentinel rect.
pub fn webview_local_dip(
    overlays: &TerminalOverlays,
    slot: u8,
    local_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Option<Vec2> {
    let rect = *overlays.rects.get(usize::from(slot))?;
    if rect.z == 0 {
        return None;
    }
    let origin_phys = Vec2::new(rect.y as f32 * cell_w_phys, rect.x as f32 * cell_h_phys);
    Some((local_phys - origin_phys) / scale_factor.max(f32::EPSILON))
}

/// The webview entity of `mount`, or `None` when no live webview has it.
pub(crate) fn webview_of_mount(
    webviews: &Query<(Entity, &Webview)>,
    mount: MountId,
) -> Option<Entity> {
    webviews
        .iter()
        .find(|(_, view)| view.mount == mount)
        .map(|(entity, _)| entity)
}

const FALLBACK_CELL_W_PHYS: f32 = 8.0;
const FALLBACK_CELL_H_PHYS: f32 = 16.0;

/// The system params the mount lifecycle needs.
#[derive(SystemParam)]
struct WebviewParams<'w, 's> {
    commands: Commands<'w, 's>,
    images: ResMut<'w, Assets<Image>>,
    // NOTE: this is the ONLY `Webview` query in this `SystemParam`. Adding a
    // second, read-only one would conflict with this mutable access and panic
    // at system init (Bevy B0001); read through `Query::get` instead.
    views: Query<'w, 's, (Entity, &'static mut Webview)>,
    children: Query<'w, 's, &'static Children>,
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

/// Spawns, resizes, and despawns webviews as the host mounts, resizes, and
/// unmounts placements. A webview that cannot be spawned is logged and
/// skipped.
fn apply_mount_event(ev: On<OrzmuxWebviewEvent>, mut webview: WebviewParams) {
    match ev.webview_event() {
        WebviewEvent::Mounted {
            pane,
            mount,
            instance,
            spec,
        } => {
            if let Err(error) = spawn_webview(&mut webview, *pane, *mount, *instance, spec) {
                tracing::error!(%error, ?mount, "webview not spawned");
            }
        }
        WebviewEvent::Resized { mount, size } => resize_webview(&mut webview, *mount, *size),
        WebviewEvent::Unmounted { mounts } => despawn_webviews(&mut webview, mounts),
        _ => {}
    }
}

/// Spawns the webview of `mount` as a `ChildOf` child of `pane` in the
/// smallest free overlay slot, with the components `spec` calls for.
///
/// `WebviewSize` is seeded to `(cols × cell_w, rows × cell_h) / scale_factor`
/// in logical px from `TerminalCellMetricsResource` and the primary window.
/// When neither exists yet (headless tests, pre-first-render) a placeholder
/// cell of 8×16 physical px at scale 1.0 is used, and the size is corrected
/// once real metrics arrive. A mount for a pane that no longer exists spawns
/// nothing.
///
/// # Errors
///
/// Returns [`WebviewError::NoFreeSlot`] when every overlay slot of `pane` is
/// occupied; nothing is spawned then.
fn spawn_webview(
    params: &mut WebviewParams,
    pane: Entity,
    mount: MountId,
    instance: InstanceId,
    spec: &MountSpec,
) -> WebviewResult {
    if params.commands.get_entity(pane).is_err() {
        tracing::debug!(?mount, "webview mount for a despawned pane dropped");
        return Ok(());
    }
    let occupied = occupied_slots(&params.children, &params.views, pane);
    let slot = smallest_free_slot(&occupied).ok_or(WebviewError::NoFreeSlot)?;
    let scale_factor = params
        .windows
        .iter()
        .next()
        .map(Window::scale_factor)
        .unwrap_or(1.0);
    let (cell_w_phys, cell_h_phys) = cell_size_phys(params.metrics.as_deref());
    let size = spec.size();
    let logical = seed_logical_size(size.rows, size.cols, cell_w_phys, cell_h_phys, scale_factor);
    let texture = WebviewTextureTarget(params.images.add(Image::default()));
    // NOTE: keep this entity free of Node / Mesh2d / Mesh3d / Sprite /
    // MaterialNode (even for debug visualization). bevy_cef's mesh/sprite
    // input paths and display-size allocators key on `With<WebviewSource>`
    // plus exactly those components; adding one double-attaches input
    // forwarding and display allocation on top of orzma's inline routing.
    // The same goes for picking components: bevy_cef writes `FocusedWebview`
    // on `Pointer<Press>`, which would bypass orzma's focus handling.
    let mut entity = params.commands.spawn((
        ChildOf(pane),
        WebviewSource::new(spec.url()),
        texture,
        WebviewSize(logical),
        Webview::new(
            spec.handle().clone(),
            instance,
            mount,
            slot,
            size.rows,
            size.cols,
        ),
        ForwardKeys::from_wire(spec.forward_keys()),
    ));
    if !spec.interactive() {
        entity.insert(NonInteractive);
    }
    if spec.bridged() {
        entity.insert(build_preload(spec.preload()));
    } else if !spec.preload().is_empty() {
        entity.insert(PreloadScripts::from(spec.preload().to_vec()));
    }
    tracing::debug!(
        handle = %spec.handle(),
        %instance,
        ?mount,
        ?pane,
        slot,
        rows = size.rows,
        cols = size.cols,
        "webview mounted"
    );
    Ok(())
}

/// Resizes the webview of `mount` to `size`; an unknown mount is ignored.
fn resize_webview(params: &mut WebviewParams, mount: MountId, size: PlacementSize) {
    if let Some((_, mut view)) = params
        .views
        .iter_mut()
        .find(|(_, view)| view.mount == mount)
    {
        let next = view.resized(size);
        view.set_if_neq(next);
    }
}

/// Despawns the webviews of `mounts`; unknown mounts are ignored.
fn despawn_webviews(params: &mut WebviewParams, mounts: &[MountId]) {
    let targets: Vec<Entity> = params
        .views
        .iter()
        .filter(|(_, view)| mounts.contains(&view.mount))
        .map(|(entity, _)| entity)
        .collect();
    for entity in targets {
        params.commands.entity(entity).try_despawn();
    }
}

/// The overlay slots the live webview children of `terminal_surface` occupy.
fn occupied_slots(
    children: &Query<&Children>,
    views: &Query<(Entity, &'static mut Webview)>,
    terminal_surface: Entity,
) -> Vec<u8> {
    let Ok(kids) = children.get(terminal_surface) else {
        return Vec::new();
    };
    kids.iter()
        .filter_map(|child| views.get(child).ok().map(|(_, view)| view.slot))
        .collect()
}

/// The smallest slot in `0..OVERLAY_SLOTS` not in `occupied`, or `None` when
/// every slot is taken.
fn smallest_free_slot(occupied: &[u8]) -> Option<u8> {
    (0..OVERLAY_SLOTS as u8).find(|slot| !occupied.contains(slot))
}

/// Physical cell pitch from the metrics resource, floored to whole physical
/// pixels and at least 1, or the 8×16 placeholder when no terminal has
/// rendered yet.
fn cell_size_phys(metrics: Option<&TerminalCellMetricsResource>) -> (f32, f32) {
    metrics
        .map(|m| {
            (
                m.metrics.advance_phys.floor().max(1.0),
                m.metrics.line_height_phys.floor().max(1.0),
            )
        })
        .unwrap_or((FALLBACK_CELL_W_PHYS, FALLBACK_CELL_H_PHYS))
}

/// The initial `WebviewSize` (logical px) for a rows×cols rect:
/// `(cols × cell_w_phys, rows × cell_h_phys) / scale_factor`.
fn seed_logical_size(
    rows: u16,
    cols: u16,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Vec2 {
    Vec2::new(f32::from(cols) * cell_w_phys, f32::from(rows) * cell_h_phys)
        / scale_factor.max(f32::EPSILON)
}

/// Recomputes every webview's `WebviewSize` from the current cell
/// metrics and primary-window scale factor, writing only when the value
/// differs.
fn sync_webview_size(
    mut sizes: Query<(&mut WebviewSize, &Webview)>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let scale_factor = windows
        .iter()
        .next()
        .map(Window::scale_factor)
        .unwrap_or(1.0);
    let (cell_w_phys, cell_h_phys) = cell_size_phys(metrics.as_deref());
    for (mut size, view) in &mut sizes {
        let next = seed_logical_size(view.rows, view.cols, cell_w_phys, cell_h_phys, scale_factor);
        size.set_if_neq(WebviewSize(next));
    }
}

/// Derives each terminal's `TerminalOverlays` from the frame-carried
/// placement list, every frame, starting from the all-sentinel default, and
/// reports each webview's first successful projection to the host as
/// `Composited`.
///
/// An id with no matching child is ignored, a mounted child whose id is
/// absent paints nothing (hidden, not unmounted), and a rect whose top
/// sits above the viewport passes through with a negative row. Each point
/// is projected with the grid's display offset; rects fully outside the
/// viewport and columns at or past the right edge are culled here.
///
/// The component is (re)inserted for every terminal that has inline
/// children OR already carries `TerminalOverlays`, so a terminal whose
/// last inline child despawned converges to all-sentinel / all-`None`
/// instead of keeping stale texture handles alive.
fn project_webview_overlays(
    mut commands: Commands,
    terminals: Query<(
        Entity,
        &TerminalView,
        Option<&Children>,
        Has<TerminalOverlays>,
    )>,
    webviews: Query<(&Webview, &WebviewTextureTarget, Has<CompositeNotified>)>,
    connection: Option<Res<OrzmuxConnection>>,
) {
    for (terminal, terminal_view, children, has_overlays) in &terminals {
        let mut overlays = TerminalOverlays::default();
        let mut has_webview_child = false;
        if let Some(kids) = children {
            for child in kids.iter() {
                let Ok((view, texture, already_notified)) = webviews.get(child) else {
                    continue;
                };
                has_webview_child = true;
                let Some(projected) = terminal_view
                    .placements
                    .iter()
                    .find(|p| p.id == view.instance)
                else {
                    continue;
                };
                let row =
                    i64::from(projected.point.line.0) + i64::from(terminal_view.display_offset);
                if row + i64::from(projected.size.rows) <= 0
                    || row >= i64::from(terminal_view.rows)
                    || u32::from(projected.point.column.0) >= u32::from(terminal_view.cols)
                {
                    continue;
                }
                let slot = usize::from(view.slot);
                if slot >= OVERLAY_SLOTS {
                    continue;
                }
                let Ok(row) = i32::try_from(row) else {
                    continue;
                };
                overlays.rects[slot] = IVec4::new(
                    row,
                    i32::from(projected.point.column.0),
                    i32::from(projected.size.rows),
                    i32::from(projected.size.cols),
                );
                overlays.textures[slot] = Some(texture.0.clone());
                if !already_notified {
                    commands.entity(child).insert(CompositeNotified);
                    if let Some(connection) = connection.as_deref() {
                        connection
                            .0
                            .send(OrzmuxCommand::Webview(WebviewCommand::Composited {
                                mount: view.mount,
                            }));
                    }
                }
            }
        }
        if has_webview_child || has_overlays {
            commands.entity(terminal).insert(overlays);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_orzma_tty_renderer::prelude::CellMetrics;
    use bevy_orzmux::prelude::OrzmuxClient;
    use crossbeam_channel::Receiver;
    use orzma_vt::prelude::{AnchoredPlacement, GridColumn, GridLine, GridPoint, MAX_PLACEMENTS};
    use orzmux::prelude::CommandSeq;

    /// The mount id the next `mount_with` hands out.
    #[derive(Resource, Default)]
    struct NextMount(u64);

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Image>>()
            .init_resource::<NextMount>()
            .add_observer(apply_mount_event);
        app
    }

    fn spawn_terminal(app: &mut App) -> Entity {
        let surface = app.world_mut().spawn(Name::new("t")).id();
        app.world_mut().flush();
        surface
    }

    /// A terminal surface and the instance the fixtures mount on it.
    fn app_with_terminal() -> (App, Entity, InstanceId) {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        (app, terminal, InstanceId(1))
    }

    /// The spec of an interactive inline page of handle `h` with the
    /// `window.orzma` bridge.
    fn inline_spec(rows: u16, cols: u16) -> MountSpec {
        MountSpec::new(
            HandleId::from("h"),
            "orzma://h/index.html",
            PlacementSize { rows, cols },
        )
        .with_bridge(true)
    }

    fn webview_event(app: &mut App, event: WebviewEvent<Entity>) {
        app.world_mut()
            .trigger(OrzmuxWebviewEvent::new(event, CommandSeq(0)));
        app.world_mut().flush();
    }

    /// Mounts `instance` on `terminal` with `spec` under a fresh mount id,
    /// as the host does for a placement that is not mounted.
    fn mount_with(
        app: &mut App,
        terminal: Entity,
        instance: InstanceId,
        spec: MountSpec,
    ) -> MountId {
        let mount = {
            let mut next = app.world_mut().resource_mut::<NextMount>();
            next.0 += 1;
            MountId::new(next.0)
        };
        webview_event(
            app,
            WebviewEvent::Mounted {
                pane: terminal,
                mount,
                instance,
                spec,
            },
        );
        mount
    }

    fn mount(app: &mut App, terminal: Entity, instance: InstanceId) -> MountId {
        mount_with(app, terminal, instance, inline_spec(10, 40))
    }

    fn resize(app: &mut App, mount: MountId, rows: u16, cols: u16) {
        webview_event(
            app,
            WebviewEvent::Resized {
                mount,
                size: PlacementSize { rows, cols },
            },
        );
    }

    fn unmount(app: &mut App, mounts: Vec<MountId>) {
        webview_event(app, WebviewEvent::Unmounted { mounts });
        app.update();
    }

    enum Op {
        Mount(InstanceId, MountId),
        Unmount(MountId),
        DespawnTerminal,
    }

    /// Queues every op from ONE system and applies the deferred commands
    /// once, the batch shape one drain of the backend's events produces.
    fn batch(app: &mut App, terminal: Entity, ops: Vec<Op>) {
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                for op in &ops {
                    match op {
                        Op::Mount(instance, mount) => commands.trigger(OrzmuxWebviewEvent::new(
                            WebviewEvent::Mounted {
                                pane: terminal,
                                mount: *mount,
                                instance: *instance,
                                spec: inline_spec(10, 40),
                            },
                            CommandSeq(0),
                        )),
                        Op::Unmount(mount) => commands.trigger(OrzmuxWebviewEvent::new(
                            WebviewEvent::Unmounted {
                                mounts: vec![*mount],
                            },
                            CommandSeq(0),
                        )),
                        Op::DespawnTerminal => commands.entity(terminal).despawn(),
                    }
                }
            })
            .expect("the batch system runs");
        app.update();
    }

    fn view_with_placements(
        rows: u16,
        cols: u16,
        placements: Vec<AnchoredPlacement>,
    ) -> TerminalView {
        view_with_placements_at(rows, cols, 0, placements)
    }

    fn view_with_placements_at(
        rows: u16,
        cols: u16,
        display_offset: u32,
        placements: Vec<AnchoredPlacement>,
    ) -> TerminalView {
        TerminalView {
            rows,
            cols,
            display_offset,
            placements,
            ..Default::default()
        }
    }

    fn run_projection(app: &mut App) {
        app.world_mut()
            .run_system_once(project_webview_overlays)
            .expect("project_webview_overlays runs");
        app.world_mut().flush();
    }

    /// The canonical 10x40 frame-carried rect at grid line 2, column 3
    /// the projection tests share.
    fn placed(id: InstanceId) -> AnchoredPlacement {
        AnchoredPlacement {
            id,
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 10, cols: 40 },
        }
    }

    fn webview_children_of(app: &App, terminal: Entity) -> Vec<Entity> {
        let world = app.world();
        world
            .get::<Children>(terminal)
            .map(|children| {
                children
                    .iter()
                    .filter(|child| world.get::<Webview>(*child).is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn live_webviews(app: &App, terminal: Entity) -> Vec<Webview> {
        webview_children_of(app, terminal)
            .into_iter()
            .filter_map(|child| app.world().get::<Webview>(child).cloned())
            .collect()
    }

    fn slot_of(app: &App, terminal: Entity, instance: InstanceId) -> Option<u8> {
        live_webviews(app, terminal)
            .into_iter()
            .find(|view| view.instance() == instance)
            .map(|view| view.slot())
    }

    /// Asserts that a mount spawns one child of its pane carrying the
    /// webview components its spec calls for.
    ///
    /// Case: a program's inline page with the `window.orzma` bridge is
    /// mounted before any terminal has rendered.
    #[test]
    fn mount_spawns_child_with_inline_components() {
        let (mut app, terminal, instance) = app_with_terminal();
        let mounted = mount(&mut app, terminal, instance);

        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 1, "mount must spawn one inline child");
        let child = children[0];
        assert_eq!(
            app.world().get::<ChildOf>(child).map(|c| c.parent()),
            Some(terminal)
        );
        assert_eq!(
            app.world().get::<Webview>(child),
            Some(&Webview::new("h".into(), instance, mounted, 0, 10, 40))
        );
        match app
            .world()
            .get::<WebviewSource>(child)
            .expect("webview must carry WebviewSource")
        {
            WebviewSource::Url(url) => assert_eq!(url, "orzma://h/index.html"),
            other => panic!("unexpected WebviewSource: {other:?}"),
        }
        assert!(app.world().get::<WebviewTextureTarget>(child).is_some());
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("webview must carry PreloadScripts");
        assert!(
            !preload.0.is_empty(),
            "a bridged webview must carry the window.orzma preload"
        );
        assert_eq!(
            app.world().get::<WebviewSize>(child),
            Some(&WebviewSize(Vec2::new(40.0 * 8.0, 10.0 * 16.0))),
            "headless seed must use the 8x16 placeholder cell at scale 1.0"
        );
        assert!(app.world().get::<NonInteractive>(child).is_none());
        assert_eq!(
            app.world().get::<ForwardKeys>(child),
            Some(&ForwardKeys::default())
        );
    }

    /// Asserts that a resize updates the reserved rect in place, keeping the
    /// entity and its slot.
    ///
    /// Case: a program re-issues its mount with a bigger rectangle after the
    /// user widens the window.
    #[test]
    fn a_resize_updates_the_reserved_rect_in_place() {
        let (mut app, terminal, instance) = app_with_terminal();
        let mounted = mount(&mut app, terminal, instance);
        let entity = webview_children_of(&app, terminal)[0];

        resize(&mut app, mounted, 12, 50);

        assert_eq!(webview_children_of(&app, terminal), vec![entity]);
        assert_eq!(
            app.world().get::<Webview>(entity),
            Some(&Webview::new("h".into(), instance, mounted, 0, 12, 50))
        );
    }

    /// Asserts that slots fill in order and that a mount past the last free
    /// slot spawns nothing.
    ///
    /// Case: programs mount one page more than the terminal has overlay
    /// slots for.
    #[test]
    fn slots_fill_in_order_and_an_over_cap_mount_is_rejected() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        for i in 0..OVERLAY_SLOTS {
            let instance = InstanceId(i as u128 + 1);
            mount(&mut app, terminal, instance);
            assert_eq!(slot_of(&app, terminal, instance), Some(i as u8));
        }
        let overflow = InstanceId(OVERLAY_SLOTS as u128 + 1);
        mount(&mut app, terminal, overflow);
        assert_eq!(webview_children_of(&app, terminal).len(), OVERLAY_SLOTS);
        assert_eq!(slot_of(&app, terminal, overflow), None);
    }

    /// Asserts that the VT's placement cap never exceeds the overlay slots,
    /// so every mount the host confirms finds a slot.
    ///
    /// Case: a program fills its terminal's placement table to the cap.
    #[test]
    fn every_placement_the_vt_accepts_gets_an_overlay_slot() {
        const { assert!(OVERLAY_SLOTS >= MAX_PLACEMENTS) };
    }

    /// Asserts that an unmount frees its slot for the next mount.
    ///
    /// Case: a program unmounts its first page and mounts a third one.
    #[test]
    fn unmount_frees_the_slot_for_the_next_mount() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let a = mount(&mut app, terminal, InstanceId(1));
        mount(&mut app, terminal, InstanceId(2));
        unmount(&mut app, vec![a]);
        assert_eq!(webview_children_of(&app, terminal).len(), 1);

        mount(&mut app, terminal, InstanceId(3));
        assert_eq!(slot_of(&app, terminal, InstanceId(3)), Some(0));
        assert_eq!(slot_of(&app, terminal, InstanceId(2)), Some(1));
    }

    /// Asserts that one unmount naming several mounts despawns each of them.
    ///
    /// Case: a program's pane closes while two of its pages are mounted.
    #[test]
    fn an_unmount_of_several_mounts_despawns_each() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let a = mount(&mut app, terminal, InstanceId(1));
        let b = mount(&mut app, terminal, InstanceId(2));
        let children = webview_children_of(&app, terminal);

        unmount(&mut app, vec![a, b]);

        assert!(webview_children_of(&app, terminal).is_empty());
        for child in children {
            assert!(app.world().get_entity(child).is_err());
        }
    }

    /// Asserts that a mount whose spec is not interactive is stamped
    /// `NonInteractive`.
    ///
    /// Case: a program mounts a clock that must not take clicks.
    #[test]
    fn non_interactive_view_is_stamped_non_interactive() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            inline_spec(10, 40).with_interactive(false),
        );
        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 1);
        assert!(app.world().get::<NonInteractive>(children[0]).is_some());
    }

    /// Asserts that two instances of one handle mount as two children in
    /// separate slots.
    ///
    /// Case: a program mints a second instance of a page it already mounted
    /// once and mounts it on the same terminal.
    #[test]
    fn two_instances_of_one_handle_mount_in_separate_slots() {
        let (mut app, terminal, first) = app_with_terminal();
        let second = InstanceId(2);
        mount(&mut app, terminal, first);
        mount(&mut app, terminal, second);
        assert_eq!(webview_children_of(&app, terminal).len(), 2);
        assert_eq!(slot_of(&app, terminal, first), Some(0));
        assert_eq!(slot_of(&app, terminal, second), Some(1));
    }

    /// Asserts that unmounting one of two instances leaves the other in its
    /// slot.
    ///
    /// Case: a program unmounts the first of two copies of its page.
    #[test]
    fn unmount_one_instance_leaves_the_other() {
        let (mut app, terminal, first) = app_with_terminal();
        let second = InstanceId(2);
        let first_mount = mount(&mut app, terminal, first);
        mount(&mut app, terminal, second);

        unmount(&mut app, vec![first_mount]);

        assert_eq!(webview_children_of(&app, terminal).len(), 1);
        assert_eq!(slot_of(&app, terminal, first), None);
        assert_eq!(slot_of(&app, terminal, second), Some(1));
    }

    /// Asserts that an unmount followed by a mount of the same placement in
    /// one drain leaves exactly one webview, for the new mount, in the freed
    /// slot.
    ///
    /// Case: a program leaves the alternate screen, which tears down the
    /// placement it held there, and re-mounts the same view on the primary
    /// screen within one pump.
    #[test]
    fn an_unmount_then_mount_batch_leaves_the_new_mount_live() {
        let (mut app, terminal, instance) = app_with_terminal();
        let old = mount(&mut app, terminal, instance);
        let new = MountId::new(100);

        batch(
            &mut app,
            terminal,
            vec![Op::Unmount(old), Op::Mount(instance, new)],
        );

        let live = live_webviews(&app, terminal);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].mount(), new);
        assert_eq!(live[0].slot(), 0);
    }

    /// Asserts that two mounts in one drain take separate slots.
    ///
    /// Case: a program mounts two pages in one burst of output.
    #[test]
    fn two_mounts_in_one_batch_take_separate_slots() {
        let (mut app, terminal, _) = app_with_terminal();
        batch(
            &mut app,
            terminal,
            vec![
                Op::Mount(InstanceId(1), MountId::new(1)),
                Op::Mount(InstanceId(2), MountId::new(2)),
            ],
        );
        assert_eq!(slot_of(&app, terminal, InstanceId(1)), Some(0));
        assert_eq!(slot_of(&app, terminal, InstanceId(2)), Some(1));
    }

    /// Asserts that a mount, its unmount, and its pane's despawn in one
    /// drain leave no webview behind.
    ///
    /// Case: a program mounts its page and its shell exits in the same burst
    /// of output, so the pane closes in the drain that mounted the page.
    #[test]
    fn a_mount_in_the_drain_that_closes_its_pane_leaves_nothing() {
        let (mut app, terminal, instance) = app_with_terminal();

        batch(
            &mut app,
            terminal,
            vec![
                Op::Mount(instance, MountId::new(1)),
                Op::Unmount(MountId::new(1)),
                Op::DespawnTerminal,
            ],
        );

        let mut webviews = app.world_mut().query::<&Webview>();
        assert_eq!(webviews.iter(app.world()).count(), 0);
    }

    /// Asserts that a mount for a pane that no longer exists spawns nothing.
    ///
    /// Case: a pane's entity is despawned before a mount the host confirmed
    /// for it is applied.
    #[test]
    fn a_mount_for_a_despawned_pane_spawns_nothing() {
        let (mut app, terminal, instance) = app_with_terminal();
        app.world_mut().entity_mut(terminal).despawn();
        mount(&mut app, terminal, instance);
        let mut webviews = app.world_mut().query::<&Webview>();
        assert_eq!(webviews.iter(app.world()).count(), 0);
    }

    /// Asserts that a resized mount keeps painting in its slot once the frame
    /// list carries the new geometry.
    ///
    /// Case: a program re-issues its mount at a new position and the VT
    /// supersedes the reservation in place.
    #[test]
    fn a_resized_mount_keeps_painting_in_its_slot() {
        let (mut app, terminal, instance) = app_with_terminal();
        let mounted = mount(&mut app, terminal, instance);
        resize(&mut app, mounted, 12, 40);
        assert_eq!(webview_children_of(&app, terminal).len(), 1);
        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements(
                24,
                80,
                vec![AnchoredPlacement {
                    id: instance,
                    point: GridPoint {
                        line: GridLine(5),
                        column: GridColumn(0),
                    },
                    size: PlacementSize { rows: 12, cols: 40 },
                }],
            ));
        run_projection(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(5, 0, 12, 40)
        );
    }

    /// Asserts that an unmount despawns exactly the named mounts and ignores
    /// unknown ones.
    ///
    /// Case: the scrollback trims past one page's row while another page
    /// further down stays alive.
    #[test]
    fn an_unmount_despawns_by_mount_and_ignores_unknowns() {
        let (mut app, terminal, memo) = app_with_terminal();
        let clock = InstanceId(2);
        let memo_mount = mount(&mut app, terminal, memo);
        mount(&mut app, terminal, clock);

        unmount(&mut app, vec![memo_mount, MountId::new(99)]);

        assert_eq!(webview_children_of(&app, terminal).len(), 1);
        assert_eq!(slot_of(&app, terminal, clock), Some(1));
    }

    /// Asserts that a directory mount loads the `orzma://` URL of its entry
    /// and carries the `window.orzma` bridge.
    ///
    /// Case: a program registers a directory of static pages whose entry
    /// sits in a subdirectory, and mounts it.
    #[test]
    fn a_directory_mount_loads_its_entry_with_the_bridge() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            MountSpec::new(
                HandleId::from("DYN1"),
                "orzma://DYN1/docs/index.html",
                PlacementSize { rows: 10, cols: 40 },
            )
            .with_bridge(true),
        );
        let child = webview_children_of(&app, terminal)[0];
        match app.world().get::<WebviewSource>(child) {
            Some(WebviewSource::Url(url)) => assert_eq!(url, "orzma://DYN1/docs/index.html"),
            other => panic!("expected an orzma URL, got {other:?}"),
        }
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts always present via WebviewSource #[require]");
        assert!(!preload.0.is_empty());
    }

    /// Asserts that a display-only URL mount carries no scripts.
    ///
    /// Case: a program shows a remote page without the bridge or preloads.
    #[test]
    fn a_display_only_url_mount_carries_no_scripts() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            MountSpec::new(
                HandleId::from("disp"),
                "https://example.com",
                PlacementSize { rows: 10, cols: 40 },
            ),
        );
        let child = webview_children_of(&app, terminal)[0];
        match app.world().get::<WebviewSource>(child) {
            Some(WebviewSource::Url(url)) => assert_eq!(url, "https://example.com"),
            other => panic!("unexpected WebviewSource: {other:?}"),
        }
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts always present via WebviewSource #[require]");
        assert!(preload.0.is_empty());
    }

    /// Asserts that a bridged URL mount carries the bridge scripts.
    ///
    /// Case: a TUI browser shows a remote page and opts into the bridge to
    /// hear about its navigations.
    #[test]
    fn a_bridged_url_mount_carries_the_bridge() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            MountSpec::new(
                HandleId::from("appv"),
                "https://app.example.com",
                PlacementSize { rows: 10, cols: 40 },
            )
            .with_bridge(true),
        );
        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert!(!preload.0.is_empty());
    }

    /// Asserts that a bridged inline mount runs the program's preload after
    /// the bridge.
    ///
    /// Case: a program registers an inline page with a preload that uses
    /// `window.orzma`.
    #[test]
    fn mount_bridged_inline_appends_user_preload_after_bridge() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            inline_spec(10, 40).with_preload(vec!["window.USER = 1;".into()]),
        );
        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert!(preload.0.len() >= 2, "bridge + user script");
        assert_eq!(
            preload.0.last().map(String::as_str),
            Some("window.USER = 1;")
        );
    }

    /// Asserts that a bridged URL mount runs the program's preload after the
    /// bridge.
    ///
    /// Case: a TUI browser injects a helper script into the remote page it
    /// bridges.
    #[test]
    fn mount_bridged_url_appends_user_preload_after_bridge() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            MountSpec::new(
                HandleId::from("u"),
                "https://app.example.com",
                PlacementSize { rows: 10, cols: 40 },
            )
            .with_bridge(true)
            .with_preload(vec!["window.USER = 1;".into()]),
        );
        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert_eq!(
            preload.0.last().map(String::as_str),
            Some("window.USER = 1;")
        );
    }

    /// Asserts that a display-only URL mount with a preload carries only the
    /// program's scripts.
    ///
    /// Case: a program shows a remote page with a style tweak but without the
    /// bridge.
    #[test]
    fn mount_display_only_url_with_preload_injects_user_scripts_only() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount_with(
            &mut app,
            terminal,
            instance,
            MountSpec::new(
                HandleId::from("disp"),
                "https://example.com",
                PlacementSize { rows: 10, cols: 40 },
            )
            .with_preload(vec!["window.USER = 1;".into()]),
        );
        let child = webview_children_of(&app, terminal)[0];
        assert_eq!(
            app.world()
                .get::<PreloadScripts>(child)
                .expect("PreloadScripts present")
                .0,
            vec!["window.USER = 1;".to_string()]
        );
    }

    fn app_with_commands() -> (App, Receiver<(CommandSeq, OrzmuxCommand)>) {
        let (client, _events, commands) = OrzmuxClient::detached();
        let mut app = make_test_app();
        app.insert_resource(OrzmuxConnection(client));
        (app, commands)
    }

    fn composited(commands: &Receiver<(CommandSeq, OrzmuxCommand)>) -> Vec<MountId> {
        commands
            .try_iter()
            .filter_map(|(_, command)| match command {
                OrzmuxCommand::Webview(WebviewCommand::Composited { mount }) => Some(mount),
                _ => None,
            })
            .collect()
    }

    /// Asserts that a webview's first successful projection stamps
    /// `CompositeNotified` and reports `Composited` for its mount once.
    ///
    /// Case: a page paints for the first time after its mount while its
    /// program listens for compositing pushes.
    #[test]
    fn the_first_projection_reports_composited() {
        let (mut app, commands) = app_with_commands();
        let terminal = app
            .world_mut()
            .spawn(view_with_placements(24, 80, vec![placed(InstanceId(1))]))
            .id();
        spawn_projection_child(&mut app, terminal, 0, InstanceId(1));

        run_projection(&mut app);

        let child = webview_children_of(&app, terminal)[0];
        assert!(app.world().get::<CompositeNotified>(child).is_some());
        assert_eq!(composited(&commands), vec![MountId::new(1)]);
    }

    /// Asserts that projecting an already reported webview reports nothing.
    ///
    /// Case: the same page keeps painting frame after frame.
    #[test]
    fn a_second_projection_reports_nothing() {
        let (mut app, commands) = app_with_commands();
        let terminal = app
            .world_mut()
            .spawn(view_with_placements(24, 80, vec![placed(InstanceId(1))]))
            .id();
        spawn_projection_child(&mut app, terminal, 0, InstanceId(1));

        run_projection(&mut app);
        assert_eq!(composited(&commands).len(), 1);
        run_projection(&mut app);
        assert!(composited(&commands).is_empty());
    }

    #[test]
    fn seed_logical_size_divides_physical_cells_by_scale() {
        assert_eq!(
            seed_logical_size(10, 40, 8.0, 16.0, 2.0),
            Vec2::new(160.0, 80.0)
        );
        assert_eq!(
            seed_logical_size(10, 40, 8.0, 16.0, 1.0),
            Vec2::new(320.0, 160.0)
        );
    }

    #[test]
    fn cell_size_phys_falls_back_without_metrics() {
        assert_eq!(
            cell_size_phys(None),
            (FALLBACK_CELL_W_PHYS, FALLBACK_CELL_H_PHYS)
        );
    }

    fn spawn_projection_child(
        app: &mut App,
        terminal: Entity,
        slot: u8,
        instance: InstanceId,
    ) -> Handle<Image> {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.world_mut().spawn((
            ChildOf(terminal),
            Webview::new(
                format!("view-{slot}").into(),
                instance,
                MountId::new(u64::from(slot) + 1),
                slot,
                10,
                40,
            ),
            WebviewTextureTarget(handle.clone()),
        ));
        handle
    }

    fn overlays_of(app: &App, terminal: Entity) -> &TerminalOverlays {
        app.world()
            .get::<TerminalOverlays>(terminal)
            .expect("projection must insert TerminalOverlays on the terminal")
    }

    fn assert_all_sentinel(overlays: &TerminalOverlays) {
        assert_eq!(
            overlays.rects,
            [IVec4::ZERO; OVERLAY_SLOTS],
            "every rect must stay at the rows == 0 sentinel"
        );
        assert!(
            overlays.textures.iter().all(Option::is_none),
            "every texture slot must stay None"
        );
    }

    /// Asserts that a projected child's texture handle lands in its
    /// own overlay slot while every other slot stays sentinel.
    ///
    /// Case: a single webview occupying slot 2 projects from the
    /// frame-carried list while the remaining slots are unoccupied.
    #[test]
    fn texture_handle_lands_in_the_childs_slot() {
        let mut app = make_test_app();
        let terminal = app
            .world_mut()
            .spawn(view_with_placements(24, 80, vec![placed(InstanceId(1))]))
            .id();
        let handle = spawn_projection_child(&mut app, terminal, 2, InstanceId(1));

        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(
            overlays.textures[2].as_ref().map(Handle::id),
            Some(handle.id()),
            "the child's texture handle must land in ITS slot"
        );
        assert_ne!(overlays.rects[2], IVec4::ZERO);
        for slot in [0, 1, 3] {
            assert!(overlays.textures[slot].is_none());
            assert_eq!(overlays.rects[slot], IVec4::ZERO);
        }
    }

    /// Asserts that two children with distinct instances each project
    /// into their own slot with their own texture handle.
    ///
    /// Case: two webview instances are mounted side by side and the
    /// same frame lists both rects.
    #[test]
    fn projection_draws_two_instances_in_their_own_slots() {
        let mut app = make_test_app();
        let terminal = app
            .world_mut()
            .spawn(view_with_placements(
                24,
                80,
                vec![placed(InstanceId(1)), placed(InstanceId(2))],
            ))
            .id();
        let h0 = spawn_projection_child(&mut app, terminal, 0, InstanceId(1));
        let h1 = spawn_projection_child(&mut app, terminal, 1, InstanceId(2));

        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(
            overlays.textures[0].as_ref().map(Handle::id),
            Some(h0.id()),
            "slot 0 must carry the first instance's texture"
        );
        assert_eq!(
            overlays.textures[1].as_ref().map(Handle::id),
            Some(h1.id()),
            "slot 1 must carry the second instance's texture"
        );
        assert_ne!(overlays.rects[0], IVec4::ZERO);
        assert_ne!(overlays.rects[1], IVec4::ZERO);
    }

    /// Asserts that after an unmount-all the next projection converges
    /// the overlays back to the all-sentinel state.
    ///
    /// Case: a program tears down every inline webview while the
    /// previous frame's overlay rects are still applied.
    #[test]
    fn stale_overlays_clear_after_unmount_all() {
        let (mut app, terminal, instance) = app_with_terminal();

        let mounted = mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements(24, 80, vec![placed(instance)]));
        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_ne!(overlays.rects[0], IVec4::ZERO);
        assert!(overlays.textures[0].is_some());

        unmount(&mut app, vec![mounted]);
        run_projection(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));
    }

    /// Asserts that a placement rect fully above, fully below, or
    /// anchored at or past the right edge of the viewport is culled to
    /// the sentinel.
    ///
    /// Case: a webview's frame-carried rect scrolls entirely off the top
    /// or bottom of the viewport, or its anchored column lands at or
    /// past the terminal's last column.
    #[test]
    fn projection_culls_fully_outside_rects() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount(&mut app, terminal, instance);

        for point in [
            GridPoint {
                line: GridLine(-20),
                column: GridColumn(0),
            },
            GridPoint {
                line: GridLine(30),
                column: GridColumn(0),
            },
            GridPoint {
                line: GridLine(2),
                column: GridColumn(80),
            },
        ] {
            app.world_mut()
                .entity_mut(terminal)
                .insert(view_with_placements(
                    24,
                    80,
                    vec![AnchoredPlacement {
                        id: instance,
                        point,
                        size: PlacementSize { rows: 6, cols: 10 },
                    }],
                ));
            run_projection(&mut app);
            let overlays = overlays_of(&app, terminal);
            assert_eq!(
                overlays.rects[0],
                IVec4::ZERO,
                "a rect outside the viewport must be culled: {point:?}"
            );
            assert!(overlays.textures[0].is_none());
        }
    }

    /// Asserts that a scrolled viewport moves a placement's rect down by
    /// the display offset.
    ///
    /// Case: the user scrolls the terminal back over output that carries a
    /// mounted webview, and the rect has to stay on the text it was
    /// anchored to.
    #[test]
    fn a_scrolled_viewport_moves_the_rect_down() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount(&mut app, terminal, instance);

        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements_at(
                24,
                80,
                3,
                vec![AnchoredPlacement {
                    id: instance,
                    point: GridPoint {
                        line: GridLine(-2),
                        column: GridColumn(0),
                    },
                    size: PlacementSize { rows: 6, cols: 10 },
                }],
            ));
        run_projection(&mut app);
        assert_eq!(overlays_of(&app, terminal).rects[0].x, 1);
    }

    /// Asserts that a placement anchored at the last valid column (`cols
    /// - 1`) still projects instead of being culled.
    ///
    /// Case: a webview's anchored column sits in the terminal's
    /// rightmost cell when the frame is captured.
    #[test]
    fn projection_keeps_rect_anchored_at_last_valid_column() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements(
                24,
                80,
                vec![AnchoredPlacement {
                    id: instance,
                    point: GridPoint {
                        line: GridLine(2),
                        column: GridColumn(79),
                    },
                    size: PlacementSize { rows: 10, cols: 10 },
                }],
            ));

        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(
            overlays.rects[0],
            IVec4::new(2, 79, 10, 10),
            "a rect anchored at the last valid column (cols - 1) must project, not cull"
        );
    }

    /// Asserts that a frame-carried placement is written through to the
    /// overlay slot verbatim, including a negative row.
    ///
    /// Case: a mounted webview's rect sticks partway above the viewport
    /// after the user scrolls.
    #[test]
    fn projection_passes_the_placement_rect_through() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements(
                24,
                80,
                vec![AnchoredPlacement {
                    id: instance,
                    point: GridPoint {
                        line: GridLine(-2),
                        column: GridColumn(4),
                    },
                    size: PlacementSize { rows: 6, cols: 20 },
                }],
            ));
        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(overlays.rects[0], IVec4::new(-2, 4, 6, 20));
        assert!(overlays.textures[0].is_some());
    }

    /// Asserts that a mounted webview whose instance is absent from the
    /// list paints nothing while its entity survives.
    ///
    /// Case: the webview scrolls fully out of the viewport; the VT
    /// omits it from the frame list without unmounting it.
    #[test]
    fn projection_hides_a_placement_absent_from_the_list() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements(24, 80, vec![]));
        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(overlays.rects[0], IVec4::ZERO);
        assert_eq!(webview_children_of(&app, terminal).len(), 1);
    }

    /// Asserts that a listed instance with no matching child is ignored.
    ///
    /// Case: the frame carrying a fresh mount's placement is applied a
    /// Bevy tick before the mount signal's observer runs.
    #[test]
    fn projection_ignores_an_unknown_instance() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        app.world_mut()
            .entity_mut(terminal)
            .insert(view_with_placements(
                24,
                80,
                vec![AnchoredPlacement {
                    id: InstanceId(9),
                    point: GridPoint {
                        line: GridLine(1),
                        column: GridColumn(1),
                    },
                    size: PlacementSize { rows: 2, cols: 2 },
                }],
            ));
        run_projection(&mut app);
        assert!(app.world().get::<TerminalOverlays>(terminal).is_none());
    }

    /// Asserts that mount-then-frame and frame-then-mount converge to
    /// the same painted overlay.
    ///
    /// Case: signal and frame arrival interleave differently across
    /// coalescer windows for the same mount.
    #[test]
    fn mount_and_frame_order_converge() {
        let rect = |instance| AnchoredPlacement {
            id: instance,
            point: GridPoint {
                line: GridLine(3),
                column: GridColumn(2),
            },
            size: PlacementSize { rows: 10, cols: 40 },
        };
        let (mut first, mounted_first, instance_a) = app_with_terminal();
        mount(&mut first, mounted_first, instance_a);
        first
            .world_mut()
            .entity_mut(mounted_first)
            .insert(view_with_placements(24, 80, vec![rect(instance_a)]));
        run_projection(&mut first);
        let rect_a = first
            .world()
            .get::<TerminalOverlays>(mounted_first)
            .expect("overlays")
            .rects[0];

        let (mut second, framed_first, instance_b) = app_with_terminal();
        second
            .world_mut()
            .entity_mut(framed_first)
            .insert(view_with_placements(24, 80, vec![rect(instance_b)]));
        run_projection(&mut second);
        assert!(
            second
                .world()
                .get::<TerminalOverlays>(framed_first)
                .is_none()
        );
        mount(&mut second, framed_first, instance_b);
        run_projection(&mut second);
        let rect_b = second
            .world()
            .get::<TerminalOverlays>(framed_first)
            .expect("overlays")
            .rects[0];
        assert_eq!(rect_a, IVec4::new(3, 2, 10, 40));
        assert_eq!(rect_a, rect_b);
    }

    /// Asserts that a metrics change recomputes a mounted webview's size at
    /// the new cell pitch.
    ///
    /// Case: the first frame renders after a page mounted, replacing the
    /// placeholder cell size with the real font metrics.
    #[test]
    fn size_sync_updates_webview_size_when_metrics_change() {
        let (mut app, terminal, instance) = app_with_terminal();
        mount(&mut app, terminal, instance);
        let child = webview_children_of(&app, terminal)[0];
        assert_eq!(
            app.world().get::<WebviewSize>(child),
            Some(&WebviewSize(Vec2::new(320.0, 160.0))),
            "the metrics-less mount must seed from the 8x16 placeholder"
        );

        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 10.0,
                line_height_phys: 20.0,
                ascent_phys: 15.0,
                descent_phys: 5.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 24,
        });
        app.world_mut().run_system_once(sync_webview_size).unwrap();

        assert_eq!(
            app.world().get::<WebviewSize>(child),
            Some(&WebviewSize(Vec2::new(400.0, 200.0))),
            "size sync must recompute the 40x10-cell rect at the real 10x20 px pitch"
        );
    }

    #[derive(Resource, Default)]
    struct SizeChangeProbe(bool);

    fn probe_webview_size_changed(
        mut probe: ResMut<SizeChangeProbe>,
        sizes: Query<Ref<WebviewSize>>,
    ) {
        probe.0 = sizes.iter().any(|size| size.is_changed());
    }

    /// Asserts that the size sync leaves `WebviewSize` unflagged when nothing
    /// it reads changed.
    ///
    /// Case: a page stays mounted across frames while the window and the
    /// font stay the same.
    #[test]
    fn size_sync_is_quiescent_when_nothing_changed() {
        let (mut app, terminal, instance) = app_with_terminal();
        app.init_resource::<SizeChangeProbe>();
        app.add_systems(
            Update,
            (sync_webview_size, probe_webview_size_changed).chain(),
        );
        mount(&mut app, terminal, instance);

        app.update();
        assert!(
            app.world().resource::<SizeChangeProbe>().0,
            "the first update after mount must see the freshly-added WebviewSize as changed"
        );

        app.update();
        assert!(
            !app.world().resource::<SizeChangeProbe>().0,
            "a second run with identical inputs must not change-flag WebviewSize"
        );
        let child = webview_children_of(&app, terminal)[0];
        assert_eq!(
            app.world().get::<WebviewSize>(child),
            Some(&WebviewSize(Vec2::new(320.0, 160.0))),
            "the value must stay at the placeholder seed"
        );
    }

    // Hit-test fixtures use an 8x16 physical-pixel cell pitch throughout.
    const HIT_CELL_W: f32 = 8.0;
    const HIT_CELL_H: f32 = 16.0;

    fn spawn_hit_child(app: &mut App, terminal: Entity, slot: u8, non_interactive: bool) -> Entity {
        let child = app
            .world_mut()
            .spawn((
                ChildOf(terminal),
                Webview::new(
                    format!("view-{slot}").into(),
                    InstanceId(u128::from(slot) + 1),
                    MountId::new(u64::from(slot) + 1),
                    slot,
                    10,
                    40,
                ),
            ))
            .id();
        if non_interactive {
            app.world_mut().entity_mut(child).insert(NonInteractive);
        }
        child
    }

    fn overlays_with(rects: &[(usize, IVec4)]) -> TerminalOverlays {
        let mut overlays = TerminalOverlays::default();
        for (slot, rect) in rects {
            overlays.rects[*slot] = *rect;
        }
        overlays
    }

    fn run_hit(
        app: &mut App,
        overlays: TerminalOverlays,
        terminal: Entity,
        local_phys: Vec2,
        scale: f32,
    ) -> Option<WebviewHit> {
        app.world_mut()
            .run_system_once(
                move |children: Query<&Children>,
                      webviews: Query<(&Webview, Has<NonInteractive>)>| {
                    webview_hit_at(
                        &children, &webviews, &overlays, terminal, local_phys, HIT_CELL_W,
                        HIT_CELL_H, scale,
                    )
                },
            )
            .unwrap()
    }

    #[test]
    fn hit_inside_active_rect_returns_child_and_dip() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        let child = spawn_hit_child(&mut app, terminal, 0, false);
        // Rect rows 2..12, cols 3..43 → phys y 32..192, x 24..344.
        let overlays = overlays_with(&[(0, IVec4::new(2, 3, 10, 40))]);

        let hit = run_hit(&mut app, overlays, terminal, Vec2::new(100.0, 100.0), 1.0);
        assert_eq!(
            hit,
            Some(WebviewHit {
                child,
                local_dip: Vec2::new(100.0 - 24.0, 100.0 - 32.0),
            }),
            "a point inside the rect must hit the slot's child with rect-relative DIP"
        );
    }

    #[test]
    fn hit_misses_outside_the_rect() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        spawn_hit_child(&mut app, terminal, 0, false);
        let overlays = overlays_with(&[(0, IVec4::new(2, 3, 10, 40))]);

        assert_eq!(
            run_hit(&mut app, overlays, terminal, Vec2::new(400.0, 300.0), 1.0),
            None,
            "a point outside every rect must miss"
        );
    }

    #[test]
    fn hit_maps_each_slot_to_its_own_child() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        let _slot0 = spawn_hit_child(&mut app, terminal, 0, false);
        let slot1 = spawn_hit_child(&mut app, terminal, 1, false);
        let overlays = overlays_with(&[(0, IVec4::new(0, 0, 2, 2)), (1, IVec4::new(4, 4, 2, 2))]);

        // (36, 72) → col 4, row 4: inside slot 1's rect only.
        let hit = run_hit(&mut app, overlays, terminal, Vec2::new(36.0, 72.0), 1.0)
            .expect("the point lies inside slot 1's rect");
        assert_eq!(
            hit.child, slot1,
            "the hit must resolve slot → child via Webview.slot"
        );
        assert_eq!(hit.local_dip, Vec2::new(36.0 - 32.0, 72.0 - 64.0));
    }

    #[test]
    fn hit_skips_non_interactive_children() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        spawn_hit_child(&mut app, terminal, 0, true);
        let overlays = overlays_with(&[(0, IVec4::new(2, 3, 10, 40))]);

        assert_eq!(
            run_hit(&mut app, overlays, terminal, Vec2::new(100.0, 100.0), 1.0),
            None,
            "a NonInteractive child must be invisible to the hit-test"
        );
    }

    #[test]
    fn hit_dip_divides_physical_offset_by_scale_factor() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        spawn_hit_child(&mut app, terminal, 0, false);
        let overlays = overlays_with(&[(0, IVec4::new(2, 3, 10, 40))]);

        let hit = run_hit(&mut app, overlays, terminal, Vec2::new(100.0, 100.0), 2.0)
            .expect("the point lies inside the rect regardless of scale");
        assert_eq!(
            hit.local_dip,
            Vec2::new((100.0 - 24.0) / 2.0, (100.0 - 32.0) / 2.0),
            "DIP must be the physical rect offset divided by the scale factor"
        );
    }

    #[test]
    fn hit_ignores_sentinel_slots() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        spawn_hit_child(&mut app, terminal, 0, false);

        assert_eq!(
            run_hit(
                &mut app,
                TerminalOverlays::default(),
                terminal,
                Vec2::new(1.0, 1.0),
                1.0,
            ),
            None,
            "a rows == 0 sentinel slot must never match, even with a live child"
        );
    }

    #[test]
    fn hit_negative_row_rect_still_hits_in_its_visible_cells() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn_empty().id();
        let child = spawn_hit_child(&mut app, terminal, 0, false);
        // Partially scrolled above: rows -2..2 visible in viewport rows 0..2.
        let overlays = overlays_with(&[(0, IVec4::new(-2, 3, 4, 10))]);

        // (44, 24) → col 5, row 1: inside the visible remainder.
        let hit = run_hit(&mut app, overlays, terminal, Vec2::new(44.0, 24.0), 1.0)
            .expect("the visible cells of a negative-row rect must still hit");
        assert_eq!(hit.child, child);
        assert_eq!(
            hit.local_dip,
            Vec2::new(44.0 - 24.0, 24.0 - (-2.0 * HIT_CELL_H)),
            "the DIP origin lies above the viewport, so local y lands past the clipped rows"
        );
    }

    #[test]
    fn webview_local_dip_rejects_sentinel_and_out_of_range_slots() {
        let overlays = overlays_with(&[(0, IVec4::new(2, 3, 10, 40))]);
        assert_eq!(
            webview_local_dip(&overlays, 0, Vec2::new(100.0, 100.0), 8.0, 16.0, 1.0),
            Some(Vec2::new(76.0, 68.0)),
        );
        assert_eq!(
            webview_local_dip(&overlays, 1, Vec2::new(100.0, 100.0), 8.0, 16.0, 1.0),
            None,
            "a sentinel slot has no DIP mapping"
        );
        assert_eq!(
            webview_local_dip(&overlays, OVERLAY_SLOTS as u8, Vec2::ZERO, 8.0, 16.0, 1.0),
            None,
            "an out-of-range slot has no DIP mapping"
        );
    }
}
