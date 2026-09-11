//! Webview mount module: `ChildOf` children of a terminal surface that render a
//! registered view into the terminal's text flow. This module owns the
//! components, the mount/unmount policy executed by the `Mount` /
//! `Unmount` observers in `apc`, and the
//! `WebviewPlugin` runtime systems that keep `WebviewSize` in
//! sync with cell metrics and project placements into `TerminalOverlays`.

use super::apc::NonInteractive;
use super::render::preload::build_preload;
use crate::control_plane::{
    ConnectionWriters, HandleId, NormalizedChord, OrzmaRegistry, OrzmaSource, PushMsg, WebviewOwner,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::{Render, RenderApp, render_asset::prepare_assets};
use bevy::ui_render::PreparedUiMaterial;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::{
    FocusedWebview, PreloadScripts, WebviewGpuImageInjectSet, WebviewSize, WebviewSource,
    WebviewTextureTarget,
};
use bevy_orzma_tty_renderer::TerminalCellMetricsResource;
use bevy_orzma_tty_renderer::material::{TerminalMaterialSystems, TerminalUiMaterial};
use bevy_orzma_tty_renderer::prelude::{OVERLAY_SLOTS, TerminalOverlays};
use bevy_orzma_tty_renderer::schema::TerminalGrid;
use bevy_orzmux::prelude::{RequestTtyWebviewRemove, TtyWebviewEvictedSignal};
use orzma_vt::prelude::InstanceId;

/// The normalized forward-key chords for a mounted webview, copied from
/// its registration. Read by the focused-key filter-fill and PTY-forward
/// systems (Phase 4) off the focused child entity.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub struct ForwardKeys(pub Vec<NormalizedChord>);

/// Marks a webview entity and records its identity: the instance it was
/// mounted under, the registration handle it came from, the overlay
/// texture slot it occupies on its parent terminal, and the rectangle it
/// reserved. The owning terminal surface is NOT duplicated here — it is
/// the `ChildOf` parent, per the "no typed back-references" convention.
/// Each child's `slot` is the single source of truth for slot allocation
/// (no separate allocation table).
///
/// # Invariants
///
/// `AnchoredPlacement.size` for this instance always equals `rows` /
/// `cols` here — the VT treats a size change as a remount, so a drift
/// between the CEF surface size and the painted rect cannot arise.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Webview {
    /// The registration this webview's content came from.
    pub handle: HandleId,
    /// The host-minted instance this webview was mounted under; frames
    /// address this placement by it.
    pub instance: InstanceId,
    /// The overlay texture slot (0..`OVERLAY_SLOTS`) on the parent terminal.
    pub slot: u8,
    /// Rect height in terminal cells.
    pub rows: u16,
    /// Rect width in terminal cells.
    pub cols: u16,
}

/// Marks a bridged webview entity after it has produced its first
/// successful projection into `TerminalOverlays` and the `Compositing { active:
/// true }` push notification has been sent. Prevents duplicate start
/// notifications on subsequent frames where the same rect re-projects.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositeNotified;

/// Registers the webview runtime systems: the `WebviewSize` size sync
/// (`Update`), the per-frame projection that derives `TerminalOverlays` from
/// the frame-carried placement list (spec §5), and the render-world ordering
/// edge that keeps webview GPU texture injection ahead of the terminal
/// material's bind-group rebuild.
///
/// The projection is scheduled in `PostUpdate` before
/// `TerminalMaterialSystems::UpdateMaterial`: grid state settles during
/// `Update` (`bevy_orzmux`'s pump emits `TtyFrameSignal` there and the
/// renderer's `apply_frame` observer mirrors it into `TerminalGrid`), so
/// projecting just before the material rebuild hands the same frame's
/// overlays to the shader.
pub(crate) struct WebviewPlugin;

impl Plugin for WebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_webview_size).add_systems(
            PostUpdate,
            project_webview_overlays.before(TerminalMaterialSystems::UpdateMaterial),
        );
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
        app.add_observer(on_webview_evicted)
            .add_observer(on_webview_removed);
    }
}

/// Everything the `Mount` verb carries into `mount`: the target
/// terminal surface and the placement the VT registered.
pub(crate) struct WebviewMountContext {
    /// The requesting terminal surface — the `ChildOf` parent of the mount.
    pub terminal_surface: Entity,
    /// The instance the mount registered. A mount the VT refused never
    /// reaches here — it arrives as `TtyWebviewMountRejectedSignal`.
    pub instance: InstanceId,
    /// Rect height in terminal cells (validated 1..=200 by `orzma_vt`).
    pub rows: u16,
    /// Rect width in terminal cells (validated 1..=400 by `orzma_vt`).
    pub cols: u16,
}

/// The system params `mount` / `unmount` need, bundled so the
/// `on_webview_mount` observer gains a single extra parameter.
#[derive(SystemParam)]
pub(crate) struct WebviewParams<'w, 's> {
    commands: Commands<'w, 's>,
    images: ResMut<'w, Assets<Image>>,
    // NOTE: this is the ONLY `Webview` query in this `SystemParam`. Adding a
    // second, read-only one would conflict with this mutable access and panic
    // at system init (Bevy B0001); read through `Query::get` instead.
    views: Query<'w, 's, &'static mut Webview>,
    children: Query<'w, 's, &'static Children>,
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

/// The resolved content + trust facts for an `Omount;n=<instance>`: the URL
/// to load (an `orzma://<handle>/…` origin for `Dir`/`Inline` sources, or the
/// verbatim remote URL for a `Url` source), the input policy, and the
/// registering program's `(connection_id, handle)` for back-channel routing.
pub(crate) struct ResolvedWebviewMount {
    /// The URL to load (`WebviewSource::Url`).
    pub url: String,
    /// Whether the page receives pointer/keyboard input.
    pub interactive: bool,
    /// `(connection_id, handle)` of the registering program, used to stamp
    /// `WebviewOwner` for `window.orzma` back-channel routing. `Some` only when
    /// the registration is bridged; a display-only `Url` view leaves it `None`,
    /// which is the gate that also withholds the preload at mount.
    pub owner: Option<(u64, HandleId)>,
    /// The normalized forward-key chords copied from the registration, stamped
    /// as a `ForwardKeys` component so the focused-key systems read them off
    /// the webview entity without a registry lookup (design spec §C).
    pub forward_keys: Vec<NormalizedChord>,
    /// User-supplied preload scripts, injected after the host bridge (and as
    /// the only scripts for a display-only view).
    pub preload: Vec<String>,
}

/// Resolves a handle (already looked up from an `Omount;n=<instance>`'s
/// instance) against the `OrzmaRegistry` (Tier 1). `Dir`/`Inline` handles
/// resolve to an `orzma://<handle>/…` URL (one origin per handle); a `Url`
/// handle resolves to its verbatim remote URL. A handle resolves ONLY when
/// `requesting_surface` is its `owner_surface` — the scoping gate that stops
/// one surface from mounting another's handle. `owner` is populated only for
/// a bridged registration (a display-only `Url` view leaves it `None`).
/// Returns `None` for an unregistered or unowned handle.
pub(crate) fn resolve_mount(
    id: &HandleId,
    requesting_surface: Entity,
    dynamic: &OrzmaRegistry,
) -> Option<ResolvedWebviewMount> {
    let view = dynamic.get(id)?;
    if view.owner_surface != requesting_surface {
        return None;
    }
    let url = match &view.source {
        OrzmaSource::Dir(_) => format!("orzma://{id}/{}", view.entry),
        OrzmaSource::Inline(_) => format!("orzma://{id}/index.html"),
        OrzmaSource::Url { url, .. } => url.clone(),
    };
    let owner = view
        .source
        .is_bridged()
        .then(|| (view.connection_id, id.clone()));
    Some(ResolvedWebviewMount {
        url,
        interactive: view.interactive,
        owner,
        forward_keys: view.forward_keys.clone(),
        preload: view.preload.clone(),
    })
}

/// Mounts a registered view as a webview child of the requesting
/// terminal surface, applying the policy gates in order: a second mount
/// of an instance already live on this terminal updates its reserved
/// rectangle instead of spawning again; an unknown instance, a handle the
/// requesting surface does not own, and overlay-slot exhaustion are each a
/// `tracing::debug!` plus a reclaim of the VT-side reservation.
///
/// The parent (`ctx.terminal_surface`, the `TtyWebviewMountSignal` target) is
/// the owning pane entity: both `OrzmuxPane` (whose drained signals include the
/// APC mount) and the required `TerminalGrid` component live on that one
/// entity, so the `ChildOf` parent is also the entity `project_webview_overlays`
/// reads grid state from.
///
/// `WebviewSize` is seeded here because `bevy_cef` builds the CEF browser
/// from it at creation. The seed is `(cols × cell_w, rows × cell_h) /
/// scale_factor` in logical px from `TerminalCellMetricsResource` and the
/// primary window; when neither exists yet (headless tests, pre-first-render)
/// a placeholder cell of 8×16 physical px at scale 1.0 is used —
/// `sync_webview_size` corrects it once real metrics arrive.
pub(crate) fn mount(params: &mut WebviewParams, dynamic: &OrzmaRegistry, ctx: WebviewMountContext) {
    let live = live_webview_children(&params.children, &params.views, ctx.terminal_surface);
    if let Some(existing) = live.iter().find(|live| live.instance == ctx.instance) {
        if let Ok(mut view) = params.views.get_mut(existing.entity) {
            let next = Webview {
                handle: view.handle.clone(),
                instance: view.instance,
                slot: view.slot,
                rows: ctx.rows,
                cols: ctx.cols,
            };
            // NOTE: set_if_neq elides a no-op re-emit so an unchanged frame
            // triggers neither a projection move nor a CEF surface resize.
            view.set_if_neq(next);
        }
        return;
    }
    let Some(handle) = dynamic
        .resolve_instance(ctx.instance)
        .map(|(h, _)| h.clone())
    else {
        tracing::debug!(instance = %ctx.instance, "apc-webview: mount for an unknown instance, dropping");
        reclaim(params, ctx.terminal_surface, ctx.instance);
        return;
    };
    let Some(resolved) = resolve_mount(&handle, ctx.terminal_surface, dynamic) else {
        tracing::debug!(%handle, "apc-webview: mount for an unowned handle, dropping");
        reclaim(params, ctx.terminal_surface, ctx.instance);
        return;
    };
    let Some(slot) = smallest_free_slot(&live) else {
        tracing::debug!(%handle, "apc-webview: all inline overlay slots occupied, dropping");
        reclaim(params, ctx.terminal_surface, ctx.instance);
        return;
    };
    let scale_factor = params
        .windows
        .iter()
        .next()
        .map(Window::scale_factor)
        .unwrap_or(1.0);
    let (cell_w_phys, cell_h_phys) = cell_size_phys(params.metrics.as_deref());
    let size = seed_logical_size(ctx.rows, ctx.cols, cell_w_phys, cell_h_phys, scale_factor);
    let texture = WebviewTextureTarget(params.images.add(Image::default()));
    let source = WebviewSource::new(resolved.url);
    let webview = params.commands.spawn_empty().id();
    // NOTE: keep this entity free of Node / Mesh2d / Mesh3d / Sprite /
    // MaterialNode (even for debug visualization). bevy_cef's mesh/sprite
    // input paths and display-size allocators key on `With<WebviewSource>`
    // plus exactly those components; adding one double-attaches input
    // forwarding and display allocation on top of orzma's inline routing
    // (design spec §4 invariant).
    params.commands.entity(webview).insert((
        ChildOf(ctx.terminal_surface),
        source,
        texture,
        WebviewSize(size),
        Webview {
            handle: handle.clone(),
            instance: ctx.instance,
            slot,
            rows: ctx.rows,
            cols: ctx.cols,
        },
    ));
    if !resolved.interactive {
        params.commands.entity(webview).insert(NonInteractive);
    }
    // NOTE: the orzma bridge script (window.orzma) and WebviewOwner (the
    // inbound-call gate) are inserted only for a bridged registration; the
    // user's preload scripts ride after the bridge. A display-only view
    // (owner None) gets no bridge and no WebviewOwner, but still receives its
    // own preload scripts when it declared any.
    if let Some((connection_id, handle)) = resolved.owner {
        params.commands.entity(webview).insert((
            build_preload(&resolved.preload),
            WebviewOwner {
                connection_id,
                handle,
                instance: ctx.instance,
            },
        ));
    } else if !resolved.preload.is_empty() {
        params
            .commands
            .entity(webview)
            .insert(PreloadScripts::from(resolved.preload.clone()));
    }
    params
        .commands
        .entity(webview)
        .insert(ForwardKeys(resolved.forward_keys.clone()));
    tracing::debug!(
        %handle,
        instance = %ctx.instance,
        terminal = ?ctx.terminal_surface,
        slot,
        rows = ctx.rows,
        cols = ctx.cols,
        "apc-webview: webview mounted"
    );
}

/// Despawns the webview child(ren) of `terminal_surface` matching the
/// scope: `Some(id)` removes that one instance; `None` removes every
/// webview child, the shape a client-issued unmount-all takes. VT-side
/// evictions (history trim, reset, alternate-screen teardown, resize)
/// arrive separately as `TtyWebviewEvictedSignal` handled by
/// `on_webview_evicted`.
pub(crate) fn unmount(
    params: &mut WebviewParams,
    terminal_surface: Entity,
    instance: Option<InstanceId>,
) {
    let targets: Vec<Entity> =
        live_webview_children(&params.children, &params.views, terminal_surface)
            .into_iter()
            .filter(|live| instance.is_none_or(|id| live.instance == id))
            .map(|live| live.entity)
            .collect();
    for entity in targets {
        params.commands.entity(entity).try_despawn();
    }
}

/// Returns the webview entity that currently holds keyboard focus on
/// `active_surface`: `Some(e)` iff `FocusedWebview` points at `e`, `e` carries
/// `Webview`, and its `ChildOf` parent is the active surface. The input
/// dispatcher uses this to hoist the release-chord check, restrict the Escape
/// scroll-to-bottom pre-handler, and suppress PTY key forwarding (spec §7).
pub fn focused_webview_of(
    focused: Option<&FocusedWebview>,
    webview_parents: &Query<&ChildOf, With<Webview>>,
    active_surface: Option<Entity>,
) -> Option<Entity> {
    let candidate = focused?.0?;
    let parent = webview_parents.get(candidate).ok()?.parent();
    (Some(parent) == active_surface).then_some(candidate)
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

/// Hit-tests a terminal-local physical-pixel point against the terminal's
/// ACTIVE inline overlay rects (the same `TerminalOverlays` projection the
/// shader composites, spec §7's single coordinate source) and returns the
/// interactive child whose rect contains it.
///
/// Cell coordinates are 0-indexed (`row = floor(local_phys.y / cell_h)`,
/// column analog) — NOT the 1-indexed `cell_at_local` convention the terminal
/// click pipeline uses. `rows == 0` sentinel slots never match; a
/// partially-scrolled rect with a negative `row` origin still hits in its
/// visible cells (its DIP origin lies above the viewport, so `local_dip.y`
/// lands past the clipped rows). `NonInteractive` children are invisible to
/// the hit-test, so their rects pass through as plain terminal input.
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
/// relative to a slot's active overlay rect, WITHOUT containment checking —
/// the release leg of an in-flight inline press uses this so a pointer that
/// drifted off the rect still produces a (possibly out-of-view) release
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

const FALLBACK_CELL_W_PHYS: f32 = 8.0;
const FALLBACK_CELL_H_PHYS: f32 = 16.0;

/// Drops the reservation a refused mount left in the VT.
///
/// The VT cannot tell a minted instance from a fabricated one, so it
/// registers a placement for any syntactically valid `n=`. Without this a
/// client can exhaust the per-terminal cap with mounts the host will never
/// place.
fn reclaim(params: &mut WebviewParams, terminal: Entity, instance: InstanceId) {
    params.commands.trigger(RequestTtyWebviewRemove {
        terminal,
        instances: vec![instance],
    });
}

/// Despawns the placements named by a `TtyWebviewEvictedSignal` on the
/// signalling terminal. Unknown ids are ignored, so a re-delivered or
/// stale eviction is a no-op.
fn on_webview_evicted(
    event: On<TtyWebviewEvictedSignal>,
    mut commands: Commands,
    children: Query<&Children>,
    views: Query<&Webview>,
) {
    let Ok(kids) = children.get(event.terminal) else {
        return;
    };
    for child in kids.iter() {
        if let Ok(view) = views.get(child)
            && event.placements.contains(&view.instance)
        {
            commands.entity(child).try_despawn();
        }
    }
}

/// One live webview child of a terminal surface: the entity plus the
/// identity the mount and unmount gates match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveWebview {
    entity: Entity,
    instance: InstanceId,
    slot: u8,
}

/// The live webview children of a terminal surface.
fn live_webview_children(
    children: &Query<&Children>,
    views: &Query<&'static mut Webview>,
    terminal_surface: Entity,
) -> Vec<LiveWebview> {
    let Ok(kids) = children.get(terminal_surface) else {
        return Vec::new();
    };
    kids.iter()
        .filter_map(|child| {
            views.get(child).ok().map(|view| LiveWebview {
                entity: child,
                instance: view.instance,
                slot: view.slot,
            })
        })
        .collect()
}

/// The smallest slot in `0..OVERLAY_SLOTS` not occupied by a live child, or
/// `None` when every slot is taken.
fn smallest_free_slot(live: &[LiveWebview]) -> Option<u8> {
    (0..OVERLAY_SLOTS as u8).find(|slot| live.iter().all(|live| live.slot != *slot))
}

/// Physical cell pitch from the metrics resource (the same floor/max the
/// terminal resize path applies), or the 8×16 placeholder when no terminal
/// has rendered yet.
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
/// metrics and primary-window scale factor (spec §6.5), writing only when the
/// value differs — `bevy_cef` commits sizes to CEF on `Changed<WebviewSize>`,
/// so a spurious write each frame would re-commit (and re-create the
/// IOSurface) every frame. Exact equality suffices: the inputs are identical
/// frame-to-frame unless metrics/scale actually changed, and this math is
/// deterministic.
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
/// placement list, every frame, starting from the all-sentinel default.
///
/// The list is authoritative and declarative: an id with no matching
/// child is ignored (its mount signal has not landed yet), a mounted
/// child whose id is absent paints nothing (hidden, not unmounted), and
/// a rect whose top sits above the viewport passes through with a
/// negative row for the shader to clip. Each point is projected with the
/// grid's display offset; rects fully outside the viewport and columns
/// at or past the right edge are culled here.
///
/// The component is (re)inserted for every terminal that has inline
/// children OR already carries `TerminalOverlays`, so a terminal whose
/// last inline child despawned converges to all-sentinel / all-`None`
/// instead of keeping stale texture handles alive.
fn project_webview_overlays(
    mut commands: Commands,
    terminals: Query<(
        Entity,
        &TerminalGrid,
        Option<&Children>,
        Has<TerminalOverlays>,
    )>,
    webviews: Query<(
        &Webview,
        &WebviewTextureTarget,
        Has<CompositeNotified>,
        Option<&WebviewOwner>,
    )>,
    writers: Res<ConnectionWriters>,
) {
    for (terminal, grid, children, has_overlays) in &terminals {
        let mut overlays = TerminalOverlays::default();
        let mut has_webview_child = false;
        if let Some(kids) = children {
            for child in kids.iter() {
                let Ok((view, texture, already_notified, owner)) = webviews.get(child) else {
                    continue;
                };
                has_webview_child = true;
                let Some(projected) = grid.placements.iter().find(|p| p.id == view.instance) else {
                    continue;
                };
                let row = i64::from(projected.point.line.0) + i64::from(grid.display_offset);
                if row + i64::from(projected.size.rows) <= 0
                    || row >= i64::from(grid.rows)
                    || u32::from(projected.point.column.0) >= u32::from(grid.cols)
                {
                    continue;
                }
                let slot = usize::from(view.slot);
                if slot >= OVERLAY_SLOTS {
                    continue;
                }
                let row =
                    i32::try_from(row).expect("the cull above bounds the row to the viewport");
                overlays.rects[slot] = IVec4::new(
                    row,
                    i32::from(projected.point.column.0),
                    i32::from(projected.size.rows),
                    i32::from(projected.size.cols),
                );
                overlays.textures[slot] = Some(texture.0.clone());
                if !already_notified {
                    commands.entity(child).insert(CompositeNotified);
                    if let Some(owner) = owner {
                        let msg = serde_json::to_string(&PushMsg::Compositing {
                            handle: owner.handle.clone(),
                            instance: owner.instance.to_string(),
                            active: true,
                        })
                        .expect("PushMsg serializes infallibly");
                        writers.send(owner.connection_id, msg);
                    }
                }
            }
        }
        if has_webview_child || has_overlays {
            commands.entity(terminal).insert(overlays);
        }
    }
}

/// Sends a `Compositing { active: false }` push notification when a bridged
/// webview entity is despawned after having been notified at least once
/// (i.e., after its first successful projection). Entities that were never
/// projected (never stamped `CompositeNotified`) are silently ignored.
fn on_webview_removed(
    event: On<Remove, Webview>,
    owners: Query<(&WebviewOwner, Has<CompositeNotified>)>,
    writers: Res<ConnectionWriters>,
) {
    let Ok((owner, notified)) = owners.get(event.entity) else {
        return;
    };
    if !notified {
        return;
    }
    let msg = serde_json::to_string(&PushMsg::Compositing {
        handle: owner.handle.clone(),
        instance: owner.instance.to_string(),
        active: false,
    })
    .expect("PushMsg serializes infallibly");
    writers.send(owner.connection_id, msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::OrzmaView;
    use crate::webview::apc::{on_webview_mount, on_webview_unmount};
    use bevy::ecs::system::RunSystemOnce;
    use bevy_cef::prelude::PreloadScripts;
    use bevy_orzma_tty_renderer::CellMetrics;
    use bevy_orzmux::prelude::{
        TtyWebviewEvictedSignal, TtyWebviewMountRejectedSignal, TtyWebviewMountSignal,
        TtyWebviewUnmountSignal,
    };
    use orzma_vt::prelude::{AnchoredPlacement, GridColumn, GridLine, GridPoint, PlacementSize};

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<OrzmaRegistry>()
            .init_resource::<Assets<Image>>()
            .init_resource::<ConnectionWriters>()
            .add_observer(on_webview_mount)
            .add_observer(on_webview_unmount)
            .add_observer(on_webview_removed)
            .add_observer(on_webview_evicted);
        app
    }

    /// Registers `view` under `handle` and mints its first instance, the
    /// two-step the control plane's `register` performs.
    fn register_view(app: &mut App, handle: &str, view: OrzmaView) -> InstanceId {
        let handle = HandleId::from(handle);
        let mut registry = app.world_mut().resource_mut::<OrzmaRegistry>();
        registry.insert(handle.clone(), view);
        registry.mint_instance(&handle).expect("the handle mints")
    }

    fn mint_extra(app: &mut App, handle: &str) -> InstanceId {
        app.world_mut()
            .resource_mut::<OrzmaRegistry>()
            .mint_instance(&HandleId::from(handle))
            .expect("the handle mints")
    }

    fn inline_view(owner_surface: Entity, interactive: bool) -> OrzmaView {
        OrzmaView {
            source: OrzmaSource::Inline("<h1>x</h1>".into()),
            entry: "index.html".into(),
            interactive,
            owner_surface,
            connection_id: 1,
            forward_keys: vec![],
            preload: vec![],
            instances: Vec::new(),
        }
    }

    fn url_view(owner_surface: Entity, url: &str, bridge: bool) -> OrzmaView {
        OrzmaView {
            source: OrzmaSource::Url {
                url: url.into(),
                bridge,
            },
            entry: String::new(),
            interactive: true,
            owner_surface,
            connection_id: 1,
            forward_keys: vec![],
            preload: vec![],
            instances: Vec::new(),
        }
    }

    fn register_orzma(
        app: &mut App,
        handle: &str,
        owner_surface: Entity,
        interactive: bool,
    ) -> InstanceId {
        register_view(app, handle, inline_view(owner_surface, interactive))
    }

    fn register_url(
        app: &mut App,
        handle: &str,
        owner_surface: Entity,
        url: &str,
        bridge: bool,
    ) -> InstanceId {
        register_view(app, handle, url_view(owner_surface, url, bridge))
    }

    fn spawn_terminal(app: &mut App) -> Entity {
        let surface = app.world_mut().spawn(Name::new("t")).id();
        app.world_mut().flush();
        surface
    }

    /// A terminal surface with one inline registration ("h") whose first
    /// instance is already minted.
    fn app_with_registration() -> (App, Entity, InstanceId) {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_orzma(&mut app, "h", terminal, true);
        (app, terminal, instance)
    }

    fn mount(app: &mut App, terminal: Entity, instance: InstanceId) {
        mount_sized(app, terminal, instance, 10, 40);
    }

    fn mount_sized(app: &mut App, terminal: Entity, instance: InstanceId, rows: u16, cols: u16) {
        app.world_mut().trigger(TtyWebviewMountSignal {
            terminal,
            instance,
            size: PlacementSize { rows, cols },
        });
        app.world_mut().flush();
    }

    fn unmount(app: &mut App, terminal: Entity, instance: Option<InstanceId>) {
        app.world_mut()
            .trigger(TtyWebviewUnmountSignal { terminal, instance });
        app.world_mut().flush();
        // NOTE: despawn is deferred; a flush + update applies it.
        app.update();
    }

    enum Op {
        Mount(InstanceId),
        Evict(InstanceId),
    }

    /// Queues every op from ONE system and applies the deferred commands
    /// once, the batch shape a single VT pump produces. Triggering each op
    /// through `World::trigger` instead would flush between them and would
    /// not exercise the interleaving these signals really arrive in.
    fn batch(app: &mut App, terminal: Entity, ops: Vec<Op>) {
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                for op in &ops {
                    match op {
                        Op::Mount(id) => commands.trigger(TtyWebviewMountSignal {
                            terminal,
                            instance: *id,
                            size: PlacementSize { rows: 10, cols: 40 },
                        }),
                        Op::Evict(id) => commands.trigger(TtyWebviewEvictedSignal {
                            terminal,
                            placements: vec![*id],
                        }),
                    }
                }
            })
            .expect("the batch system runs");
        app.update();
    }

    fn grid_with_placements(
        rows: u16,
        cols: u16,
        placements: Vec<AnchoredPlacement>,
    ) -> TerminalGrid {
        grid_with_placements_at(rows, cols, 0, placements)
    }

    fn grid_with_placements_at(
        rows: u16,
        cols: u16,
        display_offset: u32,
        placements: Vec<AnchoredPlacement>,
    ) -> TerminalGrid {
        TerminalGrid {
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
            .find(|view| view.instance == instance)
            .map(|view| view.slot)
    }

    #[test]
    fn mount_spawns_child_with_inline_components() {
        let (mut app, terminal, instance) = app_with_registration();

        mount(&mut app, terminal, instance);

        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 1, "mount must spawn one inline child");
        let child = children[0];

        assert_eq!(
            app.world().get::<ChildOf>(child).map(|c| c.parent()),
            Some(terminal),
            "the webview must be a ChildOf the terminal surface"
        );
        assert_eq!(
            app.world().get::<Webview>(child),
            Some(&Webview {
                handle: "h".into(),
                instance,
                slot: 0,
                rows: 10,
                cols: 40,
            }),
        );
        match app
            .world()
            .get::<WebviewSource>(child)
            .expect("webview must carry WebviewSource")
        {
            WebviewSource::Url(url) => assert_eq!(url, "orzma://h/index.html"),
            other => panic!("unexpected WebviewSource: {other:?}"),
        }
        assert!(
            app.world().get::<WebviewTextureTarget>(child).is_some(),
            "webview must carry a headless WebviewTextureTarget"
        );
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("webview must carry PreloadScripts");
        assert!(
            !preload.0.is_empty(),
            "an inline (bridged) webview must carry the populated window.orzma preload"
        );
        assert_eq!(
            app.world().get::<WebviewSize>(child),
            Some(&WebviewSize(Vec2::new(40.0 * 8.0, 10.0 * 16.0))),
            "headless seed must use the 8x16 placeholder cell at scale 1.0"
        );
        assert!(
            app.world().get::<NonInteractive>(child).is_none(),
            "an interactive view must not be stamped NonInteractive"
        );
    }

    /// Asserts that a second mount of a live instance resizes it in place
    /// rather than spawning a second child or reloading the page.
    ///
    /// Case: a program re-issues its mount with a bigger rectangle after
    /// the user widens the window.
    #[test]
    fn duplicate_mount_updates_the_reserved_rect_in_place() {
        let (mut app, terminal, instance) = app_with_registration();

        mount(&mut app, terminal, instance);
        let before = webview_children_of(&app, terminal);
        assert_eq!(before.len(), 1, "first mount spawns one child");
        let entity = before[0];
        let slot_before = app.world().get::<Webview>(entity).unwrap().slot;

        mount_sized(&mut app, terminal, instance, 12, 50);

        let after = webview_children_of(&app, terminal);
        assert_eq!(after.len(), 1, "re-mount must NOT spawn a second child");
        assert_eq!(
            after[0], entity,
            "re-mount must reuse the same entity (no reload)"
        );
        assert_eq!(
            app.world().get::<Webview>(entity),
            Some(&Webview {
                handle: "h".into(),
                instance,
                slot: slot_before,
                rows: 12,
                cols: 50,
            }),
            "re-mount updates the reserved rect in place and preserves the slot"
        );
    }

    #[test]
    fn slots_fill_in_order_and_an_over_cap_mount_is_rejected() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instances: Vec<InstanceId> = (0..=OVERLAY_SLOTS)
            .map(|i| register_orzma(&mut app, &format!("v{i}"), terminal, true))
            .collect();

        for (i, instance) in instances.iter().take(OVERLAY_SLOTS).enumerate() {
            mount(&mut app, terminal, *instance);
            assert_eq!(slot_of(&app, terminal, *instance), Some(i as u8));
        }

        let overflow = instances[OVERLAY_SLOTS];
        mount(&mut app, terminal, overflow);
        assert_eq!(
            webview_children_of(&app, terminal).len(),
            OVERLAY_SLOTS,
            "an over-cap mount must be rejected once all slots are taken"
        );
        assert_eq!(slot_of(&app, terminal, overflow), None);
    }

    #[test]
    fn unmount_frees_the_slot_for_the_next_mount() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let a = register_orzma(&mut app, "a", terminal, true);
        let b = register_orzma(&mut app, "b", terminal, true);
        let c = register_orzma(&mut app, "c", terminal, true);

        mount(&mut app, terminal, a);
        mount(&mut app, terminal, b);
        unmount(&mut app, terminal, Some(a));
        assert_eq!(
            webview_children_of(&app, terminal).len(),
            1,
            "unmounting one instance must despawn exactly its child"
        );

        mount(&mut app, terminal, c);
        assert_eq!(
            slot_of(&app, terminal, c),
            Some(0),
            "the freed slot 0 must be reused by the next mount"
        );
        assert_eq!(slot_of(&app, terminal, b), Some(1));
    }

    #[test]
    fn unmount_all_despawns_every_inline_child() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let a = register_orzma(&mut app, "a", terminal, true);
        let b = register_orzma(&mut app, "b", terminal, true);
        mount(&mut app, terminal, a);
        mount(&mut app, terminal, b);
        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 2);

        unmount(&mut app, terminal, None);

        assert!(
            webview_children_of(&app, terminal).is_empty(),
            "unmount-all must despawn every inline child of the terminal"
        );
        for child in children {
            assert!(
                app.world().get_entity(child).is_err(),
                "despawned inline entity must not survive"
            );
        }
    }

    #[test]
    fn non_interactive_view_is_stamped_non_interactive() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_orzma(&mut app, "hud", terminal, false);

        mount(&mut app, terminal, instance);

        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 1);
        assert!(
            app.world().get::<NonInteractive>(children[0]).is_some(),
            "a non-interactive view must carry NonInteractive"
        );
    }

    /// Asserts that a mount the VT refused spawns no child, rather than
    /// one no frame's placement list will ever address.
    ///
    /// Case: a program mounts past the terminal's placement cap, and the
    /// refusal reaches the GUI on its own signal.
    #[test]
    fn a_rejected_mount_spawns_no_child() {
        let (mut app, terminal, instance) = app_with_registration();

        app.world_mut()
            .trigger(TtyWebviewMountRejectedSignal { terminal, instance });
        app.world_mut().flush();

        assert!(
            webview_children_of(&app, terminal).is_empty(),
            "a mount the VT refused must spawn nothing"
        );
    }

    /// Asserts that a mount naming an instance the registry never minted
    /// spawns nothing.
    ///
    /// Case: a program writes a mount for an id it made up rather than one
    /// the control plane handed it.
    #[test]
    fn mount_of_an_unknown_instance_is_dropped() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, InstanceId(0xdead));

        assert!(
            webview_children_of(&app, terminal).is_empty(),
            "a mount for an unminted instance must be dropped"
        );
    }

    /// Asserts that a mount the host refuses asks the VT to drop the
    /// reservation it already registered for that instance.
    ///
    /// Case: a program mounts a fabricated instance id repeatedly, trying
    /// to fill the terminal's placement table with rects the host will
    /// never paint.
    #[test]
    fn a_refused_mount_reclaims_its_vt_reservation() {
        #[derive(Resource, Default)]
        struct Reclaimed(Vec<InstanceId>);
        let mut app = make_test_app();
        app.init_resource::<Reclaimed>().add_observer(
            |e: On<RequestTtyWebviewRemove>, mut r: ResMut<Reclaimed>| {
                r.0.extend(e.instances.iter().copied());
            },
        );
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, InstanceId(0xdead));

        assert_eq!(
            app.world().resource::<Reclaimed>().0,
            vec![InstanceId(0xdead)],
            "a refused mount must hand its reservation back to the VT"
        );
    }

    /// Asserts that two instances minted for the same handle mount as two
    /// separate children, each in its own overlay slot.
    ///
    /// Case: a program calls `new_instance` on a handle it already mounted
    /// once, then mounts the second instance on the same terminal.
    #[test]
    fn two_instances_of_one_handle_mount_in_separate_slots() {
        let (mut app, terminal, first) = app_with_registration();
        let second = mint_extra(&mut app, "h");

        mount(&mut app, terminal, first);
        mount(&mut app, terminal, second);

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            2,
            "two instances of one handle must both mount"
        );
        assert_eq!(slot_of(&app, terminal, first), Some(0));
        assert_eq!(slot_of(&app, terminal, second), Some(1));
    }

    #[test]
    fn unmount_one_instance_leaves_the_other() {
        let (mut app, terminal, first) = app_with_registration();
        let second = mint_extra(&mut app, "h");

        mount(&mut app, terminal, first);
        mount(&mut app, terminal, second);

        unmount(&mut app, terminal, Some(first));

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            1,
            "unmounting one instance must despawn exactly that instance"
        );
        assert_eq!(slot_of(&app, terminal, first), None);
        assert_eq!(slot_of(&app, terminal, second), Some(1));
    }

    #[test]
    fn slot_cap_counts_all_instances_together() {
        let (mut app, terminal, first) = app_with_registration();
        let mut instances = vec![first];
        for _ in 0..OVERLAY_SLOTS {
            instances.push(mint_extra(&mut app, "h"));
        }

        for instance in instances.iter().take(OVERLAY_SLOTS) {
            mount(&mut app, terminal, *instance);
        }
        assert_eq!(webview_children_of(&app, terminal).len(), OVERLAY_SLOTS);

        let overflow = instances[OVERLAY_SLOTS];
        mount(&mut app, terminal, overflow);
        assert_eq!(
            webview_children_of(&app, terminal).len(),
            OVERLAY_SLOTS,
            "the per-terminal slot cap counts all instances together; an over-cap mount is rejected"
        );
        assert_eq!(slot_of(&app, terminal, overflow), None);
    }

    /// Asserts that an eviction followed by a re-mount of the same
    /// instance in one signal batch leaves exactly one live webview,
    /// pinning the per-command flush `bevy_ecs` performs between queued
    /// triggers.
    ///
    /// Case: a program leaves the alternate screen — tearing down the
    /// placements it held there — and immediately re-mounts the same view
    /// on the primary screen, both within one pump.
    #[test]
    fn an_evict_then_remount_batch_leaves_one_live_webview() {
        let (mut app, terminal, id) = app_with_registration();
        batch(&mut app, terminal, vec![Op::Mount(id)]);
        batch(&mut app, terminal, vec![Op::Evict(id), Op::Mount(id)]);
        assert_eq!(live_webviews(&app, terminal).len(), 1);
    }

    /// Asserts that two mounts of the same instance in one batch collapse
    /// to a single webview holding a single overlay slot.
    ///
    /// Case: two frames' worth of output land in one pump because the app
    /// resized a view twice in quick succession.
    #[test]
    fn a_double_mount_batch_leaves_one_webview_in_one_slot() {
        let (mut app, terminal, id) = app_with_registration();
        batch(&mut app, terminal, vec![Op::Mount(id), Op::Mount(id)]);
        let live = live_webviews(&app, terminal);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].slot, 0);
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
            Webview {
                handle: format!("view-{slot}").into(),
                instance,
                slot,
                rows: 10,
                cols: 40,
            },
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
            .spawn(grid_with_placements(24, 80, vec![placed(InstanceId(1))]))
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
            .spawn(grid_with_placements(
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
        let (mut app, terminal, instance) = app_with_registration();

        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements(24, 80, vec![placed(instance)]));
        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_ne!(overlays.rects[0], IVec4::ZERO);
        assert!(overlays.textures[0].is_some());

        unmount(&mut app, terminal, None);
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
        let (mut app, terminal, instance) = app_with_registration();
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
                .insert(grid_with_placements(
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
        let (mut app, terminal, instance) = app_with_registration();
        mount(&mut app, terminal, instance);

        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements_at(
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
        let (mut app, terminal, instance) = app_with_registration();
        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements(
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
    /// after the user scrolls, and the shader clips the negative rows.
    #[test]
    fn projection_passes_the_placement_rect_through() {
        let (mut app, terminal, instance) = app_with_registration();
        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements(
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
        let (mut app, terminal, instance) = app_with_registration();
        mount(&mut app, terminal, instance);
        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements(24, 80, vec![]));
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
            .insert(grid_with_placements(
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
        let (mut first, mounted_first, instance_a) = app_with_registration();
        mount(&mut first, mounted_first, instance_a);
        first
            .world_mut()
            .entity_mut(mounted_first)
            .insert(grid_with_placements(24, 80, vec![rect(instance_a)]));
        run_projection(&mut first);
        let rect_a = first
            .world()
            .get::<TerminalOverlays>(mounted_first)
            .expect("overlays")
            .rects[0];

        let (mut second, framed_first, instance_b) = app_with_registration();
        second
            .world_mut()
            .entity_mut(framed_first)
            .insert(grid_with_placements(24, 80, vec![rect(instance_b)]));
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

    /// Asserts that a re-mount keeps its slot painting under the same
    /// instance once the frame list carries the new geometry.
    ///
    /// Case: a program re-issues `mount` for a placement it already holds
    /// and the VT supersedes the reservation in place.
    #[test]
    fn remount_keeps_painting_under_the_same_instance() {
        let (mut app, terminal, instance) = app_with_registration();
        mount(&mut app, terminal, instance);
        mount(&mut app, terminal, instance);
        assert_eq!(webview_children_of(&app, terminal).len(), 1);
        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements(
                24,
                80,
                vec![AnchoredPlacement {
                    id: instance,
                    point: GridPoint {
                        line: GridLine(5),
                        column: GridColumn(0),
                    },
                    size: PlacementSize { rows: 10, cols: 40 },
                }],
            ));
        run_projection(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(overlays.rects[0], IVec4::new(5, 0, 10, 40));
    }

    /// Asserts that an eviction signal despawns exactly the named
    /// placements and ignores unknown ids.
    ///
    /// Case: the scrollback trims past a webview's row while another
    /// webview further down stays alive.
    #[test]
    fn eviction_despawns_by_id_and_ignores_unknowns() {
        let (mut app, terminal, memo) = app_with_registration();
        let clock = register_orzma(&mut app, "clock", terminal, true);
        mount(&mut app, terminal, memo);
        mount(&mut app, terminal, clock);
        assert_eq!(webview_children_of(&app, terminal).len(), 2);
        app.world_mut().trigger(TtyWebviewEvictedSignal {
            terminal,
            placements: vec![memo, InstanceId(99)],
        });
        app.world_mut().flush();
        app.update();
        assert_eq!(webview_children_of(&app, terminal).len(), 1);
        assert_eq!(slot_of(&app, terminal, clock), Some(1));
    }

    /// Asserts that two evictions naming the same live placement in one batch
    /// despawn it once and raise no error for the second.
    ///
    /// Case: orzmd exits — leaving the alternate screen evicts its placement in
    /// the same frame the control socket's disconnect releases it.
    #[test]
    fn a_re_delivered_eviction_in_the_same_batch_is_a_no_op() {
        use crate::test_support::warnings_containing;
        let (mut app, terminal, memo) = app_with_registration();
        mount(&mut app, terminal, memo);
        let before = warnings_containing("Entity despawned").len();

        batch(&mut app, terminal, vec![Op::Evict(memo), Op::Evict(memo)]);

        assert!(webview_children_of(&app, terminal).is_empty());
        assert_eq!(
            warnings_containing("Entity despawned").len(),
            before,
            "the second eviction must not report a despawn error"
        );
    }

    #[test]
    fn size_sync_updates_webview_size_when_metrics_change() {
        let (mut app, terminal, instance) = app_with_registration();
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

    #[test]
    fn size_sync_is_quiescent_when_nothing_changed() {
        let (mut app, terminal, instance) = app_with_registration();
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
                Webview {
                    handle: format!("view-{slot}").into(),
                    instance: InstanceId(u128::from(slot) + 1),
                    slot,
                    rows: 10,
                    cols: 40,
                },
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

    fn dir_view(owner_surface: Entity, interactive: bool) -> OrzmaView {
        OrzmaView {
            source: OrzmaSource::Dir("/abs/ui".into()),
            entry: "index.html".into(),
            interactive,
            owner_surface,
            connection_id: 1,
            forward_keys: vec![],
            preload: vec![],
            instances: Vec::new(),
        }
    }

    #[test]
    fn mount_of_dynamic_handle_uses_orzma_url_and_no_bridge() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_view(&mut app, "DYN1", dir_view(terminal, true));

        mount(&mut app, terminal, instance);

        let children = webview_children_of(&app, terminal);
        assert_eq!(
            children.len(),
            1,
            "dynamic mount must spawn one inline child"
        );
        match app.world().get::<WebviewSource>(children[0]).unwrap() {
            WebviewSource::Url(u) => assert_eq!(u, "orzma://DYN1/index.html"),
            other => panic!("expected orzma URL, got {other:?}"),
        }
        let preload = app.world().get::<PreloadScripts>(children[0]).unwrap();
        assert!(
            !preload.0.iter().any(|s| s.contains("__orzmaGranted")),
            "a dynamic view must carry no capability grant / host bridge"
        );
    }

    #[test]
    fn resolve_mount_enforces_owner_surface() {
        let owner = Entity::from_bits(1);
        let other = Entity::from_bits(2);
        let handle = HandleId::from("DYNHANDLE");
        let mut dynamic = OrzmaRegistry::default();
        dynamic.insert(handle.clone(), dir_view(owner, false));

        let d = resolve_mount(&handle, owner, &dynamic).expect("dynamic resolves");
        assert_eq!(d.url, "orzma://DYNHANDLE/index.html");
        assert!(!d.interactive);

        assert!(
            resolve_mount(&handle, other, &dynamic).is_none(),
            "a handle resolves only from its owner surface"
        );
        assert!(resolve_mount(&HandleId::from("ghost"), owner, &dynamic).is_none());
    }

    #[test]
    fn resolve_mount_dynamic_inline_yields_orzma_url_via_index_html() {
        let owner = Entity::from_bits(1);
        let handle = HandleId::from("INLINEH");
        let mut dynamic = OrzmaRegistry::default();
        dynamic.insert(handle.clone(), inline_view(owner, true));
        let r = resolve_mount(&handle, owner, &dynamic).expect("inline resolves");
        assert_eq!(r.url, "orzma://INLINEH/index.html");
        assert!(r.owner.is_some());
    }

    #[test]
    fn dynamic_mount_stamps_webview_owner() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_view(
            &mut app,
            "HANDLE",
            OrzmaView {
                connection_id: 42,
                ..inline_view(terminal, true)
            },
        );

        mount(&mut app, terminal, instance);

        let children = webview_children_of(&app, terminal);
        assert_eq!(
            children.len(),
            1,
            "dynamic mount must spawn one inline child"
        );
        assert_eq!(
            app.world().get::<WebviewOwner>(children[0]),
            Some(&WebviewOwner {
                connection_id: 42,
                handle: "HANDLE".into(),
                instance,
            }),
            "dynamic mount must stamp WebviewOwner with the mounted instance"
        );
    }

    #[test]
    fn resolve_mount_url_returns_verbatim_url_and_gates_owner_on_bridge() {
        let surface = Entity::from_bits(1);
        let disp_handle = HandleId::from("disp");
        let app_handle = HandleId::from("appv");
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            disp_handle.clone(),
            OrzmaView {
                connection_id: 7,
                ..url_view(surface, "https://example.com", false)
            },
        );
        reg.insert(
            app_handle.clone(),
            OrzmaView {
                connection_id: 7,
                ..url_view(surface, "https://app.example.com", true)
            },
        );

        let disp = resolve_mount(&disp_handle, surface, &reg).expect("registered");
        assert_eq!(disp.url, "https://example.com");
        assert!(disp.owner.is_none(), "display-only url must have no owner");

        let appv = resolve_mount(&app_handle, surface, &reg).expect("registered");
        assert_eq!(appv.url, "https://app.example.com");
        assert_eq!(appv.owner, Some((7, app_handle)));
    }

    #[test]
    fn mount_url_display_only_has_no_preload_or_owner() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_url(&mut app, "disp", terminal, "https://example.com", false);

        mount(&mut app, terminal, instance);

        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 1);
        let child = children[0];
        match app
            .world()
            .get::<WebviewSource>(child)
            .expect("WebviewSource present")
        {
            WebviewSource::Url(url) => assert_eq!(url, "https://example.com"),
            other => panic!("unexpected WebviewSource: {other:?}"),
        }
        // NOTE: WebviewSource carries #[require(PreloadScripts)] in bevy_cef, so
        // the component is always inserted (Default = empty vec) by Bevy's
        // required-component machinery. The gate for a display-only view is that
        // the orzma bridge scripts are absent (empty vec), not that the component
        // itself is absent.
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts always present via WebviewSource #[require]");
        assert!(
            preload.0.is_empty(),
            "a display-only url must carry no orzma bridge scripts"
        );
        assert!(
            app.world().get::<WebviewOwner>(child).is_none(),
            "a display-only url must carry no WebviewOwner"
        );
    }

    #[test]
    fn mount_url_bridged_has_preload_and_owner() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_url(&mut app, "appv", terminal, "https://app.example.com", true);

        mount(&mut app, terminal, instance);

        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert!(
            !preload.0.is_empty(),
            "a bridged url must carry the orzma bridge scripts"
        );
        assert_eq!(
            app.world().get::<WebviewOwner>(child),
            Some(&WebviewOwner {
                connection_id: 1,
                handle: "appv".into(),
                instance,
            }),
        );
    }

    #[test]
    fn mount_bridged_inline_appends_user_preload_after_bridge() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_view(
            &mut app,
            "h",
            OrzmaView {
                preload: vec!["window.USER = 1;".into()],
                ..inline_view(terminal, true)
            },
        );

        mount(&mut app, terminal, instance);

        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert!(preload.0.len() >= 2, "bridge + user script");
        assert_eq!(
            preload.0.last().map(String::as_str),
            Some("window.USER = 1;"),
            "the user script must come last, after the orzma bridge"
        );
    }

    #[test]
    fn mount_bridged_url_appends_user_preload_after_bridge() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_view(
            &mut app,
            "u",
            OrzmaView {
                preload: vec!["window.USER = 1;".into()],
                ..url_view(terminal, "https://app.example.com", true)
            },
        );

        mount(&mut app, terminal, instance);

        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert_eq!(
            preload.0.last().map(String::as_str),
            Some("window.USER = 1;"),
            "the user script must come last, after the bridge"
        );
    }

    #[test]
    fn mount_display_only_url_with_preload_injects_user_scripts_only() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        let instance = register_view(
            &mut app,
            "disp",
            OrzmaView {
                preload: vec!["window.USER = 1;".into()],
                ..url_view(terminal, "https://example.com", false)
            },
        );

        mount(&mut app, terminal, instance);

        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert_eq!(
            preload.0,
            vec!["window.USER = 1;".to_string()],
            "a display-only url with preload must carry only the user scripts"
        );
        assert!(
            app.world().get::<WebviewOwner>(child).is_none(),
            "a display-only url must carry no WebviewOwner even with preload"
        );
    }

    fn compositing_writers(
        connection_id: u64,
    ) -> (ConnectionWriters, crossbeam_channel::Receiver<String>) {
        use crossbeam_channel::bounded;
        let (tx, rx) = bounded(16);
        let writers = ConnectionWriters::default();
        writers.insert(connection_id, tx);
        (writers, rx)
    }

    fn spawn_owned_projection_child(
        app: &mut App,
        terminal: Entity,
        slot: u8,
        instance: InstanceId,
        connection_id: u64,
        handle: &str,
    ) -> Entity {
        let image_handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.world_mut()
            .spawn((
                ChildOf(terminal),
                Webview {
                    handle: handle.into(),
                    instance,
                    slot,
                    rows: 10,
                    cols: 40,
                },
                WebviewTextureTarget(image_handle.clone()),
                WebviewOwner {
                    connection_id,
                    handle: handle.into(),
                    instance,
                },
            ))
            .id()
    }

    /// Asserts that a child's first successful projection stamps
    /// `CompositeNotified` and sends exactly one
    /// `Compositing { active: true }` push naming its instance.
    ///
    /// Case: a bridged webview paints for the first time after its
    /// mount while the registering program listens for compositing
    /// pushes.
    #[test]
    fn first_projection_sends_compositing_start() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app
            .world_mut()
            .spawn(grid_with_placements(24, 80, vec![placed(InstanceId(1))]))
            .id();
        let entity =
            spawn_owned_projection_child(&mut app, terminal, 0, InstanceId(1), 1, "myhandle");

        run_projection(&mut app);

        assert!(
            app.world().get::<CompositeNotified>(entity).is_some(),
            "first successful projection must stamp CompositeNotified"
        );
        let msg = rx
            .try_recv()
            .expect("compositing start must be sent after first projection");
        assert_eq!(
            msg,
            format!(
                r#"{{"op":"compositing","handle":"myhandle","instance":"{}","active":true}}"#,
                InstanceId(1)
            )
        );
    }

    /// Asserts that projecting an already-notified child sends no
    /// duplicate compositing push.
    ///
    /// Case: the same webview keeps projecting frame after frame while
    /// its owner stays connected.
    #[test]
    fn second_projection_does_not_resend() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app
            .world_mut()
            .spawn(grid_with_placements(24, 80, vec![placed(InstanceId(1))]))
            .id();
        spawn_owned_projection_child(&mut app, terminal, 0, InstanceId(1), 1, "myhandle");

        run_projection(&mut app);
        let _ = rx.try_recv().expect("first projection must send start");

        run_projection(&mut app);
        assert!(
            rx.try_recv().is_err(),
            "second projection must NOT send a duplicate start"
        );
    }

    /// Asserts that despawning a child that was notified at least once
    /// sends `Compositing { active: false }` naming its instance.
    ///
    /// Case: a webview that has been painting is unmounted, and the
    /// registering program must learn that compositing ended.
    #[test]
    fn stop_observer_sends_compositing_stop_when_notified() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app
            .world_mut()
            .spawn(grid_with_placements(24, 80, vec![placed(InstanceId(1))]))
            .id();
        let child =
            spawn_owned_projection_child(&mut app, terminal, 0, InstanceId(1), 1, "myhandle");

        run_projection(&mut app);
        let _ = rx.try_recv().expect("start notification must arrive");

        app.world_mut().entity_mut(child).despawn();
        app.world_mut().flush();

        let msg = rx
            .try_recv()
            .expect("compositing stop must be sent on despawn");
        assert_eq!(
            msg,
            format!(
                r#"{{"op":"compositing","handle":"myhandle","instance":"{}","active":false}}"#,
                InstanceId(1)
            )
        );
    }

    /// Asserts that despawning a child that never projected sends no
    /// stop push.
    ///
    /// Case: a webview is mounted and torn down again before any frame
    /// lists its placement, so compositing never started.
    #[test]
    fn stop_observer_does_not_send_when_not_notified() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app
            .world_mut()
            .spawn(grid_with_placements(24, 80, vec![]))
            .id();
        let child =
            spawn_owned_projection_child(&mut app, terminal, 0, InstanceId(1), 1, "myhandle");

        app.world_mut().entity_mut(child).despawn();
        app.world_mut().flush();

        assert!(
            rx.try_recv().is_err(),
            "despawning a never-projected entity must NOT send a stop notification"
        );
    }
}
