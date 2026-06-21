//! Webview mount module: `ChildOf` children of a terminal surface that render a
//! registered view into the terminal's text flow. This module owns the
//! components, the mount/unmount policy executed by the `Mount` /
//! `Unmount` arms of `osc::on_osc_webview_request`, and the
//! `WebviewPlugin` runtime systems that keep `WebviewSize` in
//! sync with cell metrics and project placements into `TerminalOverlays`.

use super::osc::NonInteractive;
use super::render::preload::build_preload;
use crate::control_plane::{
    ConnectionWriters, DynSource, DynamicRegistry, NormalizedChord, PushMsg, WebviewOwner,
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
use ozma_tty_engine::{AnchorMode, InlineAnchor, TerminalModeChanged};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::{TerminalMaterialSystems, TerminalUiMaterial};
use ozma_tty_renderer::prelude::{OVERLAY_SLOTS, TerminalOverlays};
use ozma_tty_renderer::schema::TerminalGrid;

/// The normalized forward-key chords for a mounted webview, copied from
/// its registration. Read by the focused-key filter-fill and PTY-forward
/// systems (Phase 4) off the focused child entity.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ForwardKeys(pub(crate) Vec<NormalizedChord>);

/// Marks a webview entity and records its identity: the mounted
/// `view_id` and the overlay texture `slot` (0..`OVERLAY_SLOTS`) it occupies
/// on its parent terminal. The owning terminal surface is NOT duplicated
/// here — it is the `ChildOf` parent, per the multiplexer's "no typed
/// back-references" convention. Each child's `slot` is the single source of
/// truth for slot allocation (no separate allocation table).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Webview {
    /// The registered view id this webview was mounted from.
    pub(crate) view_id: String,
    /// The client-assigned instance id; `None` is the implicit default
    /// instance. `(view_id, instance_id)` is the per-terminal address.
    pub(crate) instance_id: Option<String>,
    /// The overlay texture slot (0..`OVERLAY_SLOTS`) on the parent terminal.
    pub(crate) slot: u8,
}

/// Where a webview sits: its anchor mode (scrollback line vs fixed
/// viewport cell), the rect extent in cells, and the VT `frame_seq` the next
/// grid emit carries (`project_webview_overlays` defers first projection until
/// the grid catches up).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WebviewPlacement {
    /// Where the rect is anchored.
    pub(crate) anchor: AnchorMode,
    /// Rect height in terminal cells.
    pub(crate) rows: u16,
    /// Rect width in terminal cells.
    pub(crate) cols: u16,
    /// The VT frame seq stamped at mount; grid frames at or after this seq
    /// (wrap-aware compare) may project the placement.
    pub(crate) frame_seq: u32,
}

/// Marks a bridged webview entity after it has produced its first
/// successful projection into `TerminalOverlays` and the `Compositing { active:
/// true }` push notification has been sent. Prevents duplicate start
/// notifications on subsequent frames where the same rect re-projects.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositeNotified;

/// Registers the webview runtime systems: the `WebviewSize` size sync
/// (`Update`), the per-frame projection that derives `TerminalOverlays` from
/// webview children (spec §5), and the render-world ordering edge that
/// keeps webview GPU texture injection ahead of the terminal material's
/// bind-group rebuild.
///
/// The projection is scheduled in `PostUpdate` before
/// `TerminalMaterialSystems::UpdateMaterial`: grid state settles during
/// `Update` (the PTY drain systems flush the `FrameSnapshot` / `FrameDelta`
/// observers there), so projecting just before the material rebuild hands the
/// same frame's overlays to the shader.
pub(super) struct WebviewPlugin;

impl Plugin for WebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_webview_size);
        app.add_systems(
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
        app.add_observer(despawn_fixed_screen_on_alt_exit);
        app.add_observer(on_placement_removed);
    }
}

/// Everything the `Mount` verb carries into `mount`: the target
/// terminal surface and the parsed verb + anchor payload.
pub(crate) struct WebviewMountContext<'a> {
    /// The requesting terminal surface — the `ChildOf` parent of the mount.
    pub(crate) terminal_surface: Entity,
    /// The registered view id to mount.
    pub(crate) view_id: &'a str,
    /// The client-assigned instance id (`None` = implicit default instance).
    pub(crate) instance_id: Option<&'a str>,
    /// Rect height in terminal cells (validated 1..=200 by `ozma_tty_engine`).
    pub(crate) rows: u16,
    /// Rect width in terminal cells (validated 1..=400 by `ozma_tty_engine`).
    pub(crate) cols: u16,
    /// The VT-stamped anchor; `None` is a policy rejection (gate 1).
    pub(crate) anchor: Option<InlineAnchor>,
}

/// The system params `mount` / `unmount` need, bundled so the
/// `on_osc_webview_request` observer gains a single extra parameter.
#[derive(SystemParam)]
pub(crate) struct WebviewParams<'w, 's> {
    commands: Commands<'w, 's>,
    images: ResMut<'w, Assets<Image>>,
    placements: Query<'w, 's, &'static mut WebviewPlacement>,
    children: Query<'w, 's, &'static Children>,
    views: Query<'w, 's, &'static Webview>,
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

/// The resolved content + trust facts for a `mount;<handle>`: the URL to
/// load (an `ozma-dyn://<handle>/…` origin for `Dir`/`Inline` sources, or the
/// verbatim remote URL for a `Url` source), the input policy, and the
/// registering program's `(connection_id, handle)` for back-channel routing.
pub(crate) struct ResolvedWebviewMount {
    /// The URL to load (`WebviewSource::Url`). `None` signals a policy rejection.
    pub(crate) url: Option<String>,
    /// Whether the page receives pointer/keyboard input.
    pub(crate) interactive: bool,
    /// `(connection_id, handle)` of the registering program, used to stamp
    /// `WebviewOwner` for `window.ozma` back-channel routing. `Some` only when
    /// the registration is bridged; a display-only `Url` view leaves it `None`,
    /// which is the gate that also withholds the preload at mount.
    pub(crate) owner: Option<(u64, String)>,
    /// The normalized forward-key chords copied from the registration, stamped
    /// as a `ForwardKeys` component so the focused-key systems read them off
    /// the webview entity without a registry lookup (design spec §C).
    pub(crate) forward_keys: Vec<NormalizedChord>,
    /// User-supplied preload scripts, injected after the host bridge (and as
    /// the only scripts for a display-only view).
    pub(crate) preload: Vec<String>,
}

/// Resolves a `mount` `<handle>` against the `DynamicRegistry` (Tier 1).
/// `Dir`/`Inline` handles resolve to an `ozma-dyn://<handle>/…` URL (one origin
/// per handle); a `Url` handle resolves to its verbatim remote URL. A handle
/// resolves ONLY when `requesting_surface` is its `owner_surface` — the scoping
/// gate that stops one surface from mounting another's handle. `owner` is
/// populated only for a bridged registration (a display-only `Url` view leaves it
/// `None`). Returns `None` for an unregistered or unowned handle.
pub(crate) fn resolve_mount(
    id: &str,
    requesting_surface: Entity,
    dynamic: &DynamicRegistry,
) -> Option<ResolvedWebviewMount> {
    let view = dynamic.get(id)?;
    if view.owner_surface != requesting_surface {
        return None;
    }
    let url = match &view.source {
        DynSource::Dir(_) => format!("ozma-dyn://{id}/{}", view.entry),
        DynSource::Inline(_) => format!("ozma-dyn://{id}/index.html"),
        DynSource::Url { url, .. } => url.clone(),
    };
    let owner = view
        .source
        .is_bridged()
        .then(|| (view.connection_id, id.to_string()));
    Some(ResolvedWebviewMount {
        url: Some(url),
        interactive: view.interactive,
        owner,
        forward_keys: view.forward_keys.clone(),
        preload: view.preload.clone(),
    })
}

/// Mounts a registered view as a webview child of the requesting
/// terminal surface, applying the policy gates in order (each rejection is a
/// `tracing::debug!` + return): missing anchor, unregistered view, duplicate
/// `view_id` on this terminal, overlay-slot exhaustion.
///
/// The parent (`ctx.terminal_surface`, the `OscWebviewRequest` target) is the
/// `TmuxPane` entity itself: `tmux_render::attach_tmux_pane_terminal` inserts
/// both the `TerminalHandle` (which emits the OSC request) and the
/// `TerminalRenderBundle` (`TerminalGrid`) onto that one entity, so the
/// `ChildOf` parent is also the entity `project_webview_overlays` reads grid
/// state from.
///
/// `WebviewSize` is seeded here because `bevy_cef` builds the CEF browser
/// from it at creation. The seed is `(cols × cell_w, rows × cell_h) /
/// scale_factor` in logical px from `TerminalCellMetricsResource` and the
/// primary window; when neither exists yet (headless tests, pre-first-render)
/// a placeholder cell of 8×16 physical px at scale 1.0 is used —
/// `sync_webview_size` corrects it once real metrics arrive.
pub(crate) fn mount(
    params: &mut WebviewParams,
    dynamic: &DynamicRegistry,
    ctx: WebviewMountContext<'_>,
) {
    let Some(anchor) = ctx.anchor else {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: mount without anchor, dropping");
        return;
    };
    let live = live_webview_children(&params.children, &params.views, ctx.terminal_surface);
    if let Some((existing, _)) = live
        .iter()
        .find(|(_, v)| v.view_id == ctx.view_id && v.instance_id.as_deref() == ctx.instance_id)
    {
        let next = WebviewPlacement {
            anchor: anchor.mode,
            rows: ctx.rows,
            cols: ctx.cols,
            frame_seq: anchor.frame_seq,
        };
        if let Ok(mut placement) = params.placements.get_mut(*existing) {
            // NOTE: set_if_neq elides a no-op re-emit so an unchanged frame
            // triggers neither a projection move nor a CEF surface resize.
            placement.set_if_neq(next);
        }
        return;
    }
    let Some(resolved) = resolve_mount(ctx.view_id, ctx.terminal_surface, dynamic) else {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: mount for unregistered or unowned id, dropping");
        return;
    };
    let Some(slot) = smallest_free_slot(&live) else {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: all inline overlay slots occupied, dropping");
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
    let Some(url) = resolved.url.as_deref() else {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: resolved mount had no url, dropping");
        return;
    };
    let source = WebviewSource::new(url);
    let webview = params.commands.spawn_empty().id();
    // NOTE: keep this entity free of Node / Mesh2d / Mesh3d / Sprite /
    // MaterialNode (even for debug visualization). bevy_cef's mesh/sprite
    // input paths and display-size allocators key on `With<WebviewSource>`
    // plus exactly those components; adding one double-attaches input
    // forwarding and display allocation on top of ozmux's inline routing
    // (design spec §4 invariant).
    params.commands.entity(webview).insert((
        ChildOf(ctx.terminal_surface),
        source,
        texture,
        WebviewSize(size),
        Webview {
            view_id: ctx.view_id.to_string(),
            instance_id: ctx.instance_id.map(str::to_string),
            slot,
        },
        WebviewPlacement {
            anchor: anchor.mode,
            rows: ctx.rows,
            cols: ctx.cols,
            frame_seq: anchor.frame_seq,
        },
    ));
    if !resolved.interactive {
        params.commands.entity(webview).insert(NonInteractive);
    }
    // NOTE: the ozma bridge script (window.ozma) and WebviewOwner (the
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
        view_id = %ctx.view_id,
        terminal = ?ctx.terminal_surface,
        slot,
        rows = ctx.rows,
        cols = ctx.cols,
        anchor = ?anchor.mode,
        "osc-webview: webview mounted"
    );
}

/// Despawns the inline child(ren) of `terminal_surface` matching the scope:
/// `(Some(vid), Some(inst))` removes that one instance; `(Some(vid), None)`
/// removes every instance of `vid`; `(None, _)` removes all inline children
/// (the VT-synthesized fold/saturation `Unmount { view_id: None }`
/// frames take this path).
pub(crate) fn unmount(
    params: &mut WebviewParams,
    terminal_surface: Entity,
    view_id: Option<&str>,
    instance_id: Option<&str>,
) {
    let targets: Vec<Entity> =
        live_webview_children(&params.children, &params.views, terminal_surface)
            .into_iter()
            .filter(|(_, v)| match (view_id, instance_id) {
                (Some(vid), Some(inst)) => {
                    v.view_id == vid && v.instance_id.as_deref() == Some(inst)
                }
                (Some(vid), None) => v.view_id == vid,
                (None, _) => true,
            })
            .map(|(entity, _)| entity)
            .collect();
    for entity in targets {
        params.commands.entity(entity).despawn();
    }
}

/// Returns the webview entity that currently holds keyboard focus on
/// `active_surface`: `Some(e)` iff `FocusedWebview` points at `e`, `e` carries
/// `Webview`, and its `ChildOf` parent is the active surface. The input
/// dispatcher uses this to hoist the release-chord check, restrict the Escape
/// scroll-to-bottom pre-handler, and suppress PTY key forwarding (spec §7).
pub(crate) fn focused_webview_of(
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
pub(crate) struct WebviewHit {
    /// The interactive webview child under the pointer.
    pub(crate) child: Entity,
    /// `(local_phys − rect_origin_phys) / scale_factor` — the pointer in
    /// webview-local DIP, the coordinate space CEF mouse events expect.
    pub(crate) local_dip: Vec2,
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
pub(crate) fn webview_hit_at(
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
pub(crate) fn webview_local_dip(
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

/// Despawns the `FixedScreen` inline children of a terminal when it leaves the
/// alternate screen. The teardown lands before the next `PostUpdate`
/// projection (the engine triggers `TerminalModeChanged` before the frame
/// trigger, and despawn commands flush at the `Update`->`PostUpdate` boundary),
/// so no stale rectangle is painted (spec section 4.6, Kitty issue #2901).
fn despawn_fixed_screen_on_alt_exit(
    event: On<TerminalModeChanged>,
    mut commands: Commands,
    children: Query<&Children>,
    placements: Query<&WebviewPlacement>,
) {
    if !event.removed.iter().any(|m| m == ALT_SCREEN_MODE) {
        return;
    }
    let Ok(kids) = children.get(event.entity) else {
        return;
    };
    for child in kids.iter() {
        if let Ok(placement) = placements.get(child)
            && matches!(placement.anchor, AnchorMode::FixedScreen { .. })
        {
            commands.entity(child).despawn();
        }
    }
}

/// The live webview children of a terminal surface.
fn live_webview_children<'a>(
    children: &Query<&Children>,
    views: &'a Query<&'static Webview>,
    terminal_surface: Entity,
) -> Vec<(Entity, &'a Webview)> {
    let Ok(kids) = children.get(terminal_surface) else {
        return Vec::new();
    };
    kids.iter()
        .filter_map(|child| views.get(child).ok().map(|view| (child, view)))
        .collect()
}

/// The smallest slot in `0..OVERLAY_SLOTS` not occupied by a live child, or
/// `None` when every slot is taken.
fn smallest_free_slot(live: &[(Entity, &Webview)]) -> Option<u8> {
    (0..OVERLAY_SLOTS as u8).find(|slot| live.iter().all(|(_, v)| v.slot != *slot))
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
    mut sizes: Query<(&mut WebviewSize, &WebviewPlacement)>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let scale_factor = windows
        .iter()
        .next()
        .map(Window::scale_factor)
        .unwrap_or(1.0);
    let (cell_w_phys, cell_h_phys) = cell_size_phys(metrics.as_deref());
    for (mut size, placement) in &mut sizes {
        let next = seed_logical_size(
            placement.rows,
            placement.cols,
            cell_w_phys,
            cell_h_phys,
            scale_factor,
        );
        size.set_if_neq(WebviewSize(next));
    }
}

/// The wire mode string `ozma_tty_engine`'s `mode_diff` emits for
/// `TermMode::ALT_SCREEN`.
const ALT_SCREEN_MODE: &str = "alt-screen";

/// Derives each terminal's `TerminalOverlays` from its live webview
/// children, every frame, starting from the all-sentinel default (spec §5).
///
/// Per-child projection rules, in order:
/// 1. Seq-hold: a placement is skipped until the grid's `last_seq` reaches the
///    mount-stamped `frame_seq` (wrap-aware compare).
/// 2. Screen-mode gating: a `Scrollback` placement projects only on the primary
///    screen (hidden while on the alternate screen); a `FixedScreen` placement
///    projects only on the alternate screen.
/// 3. `Scrollback`: `viewport_row = line - (history_base + history_size -
///    display_offset)`. `FixedScreen`: `viewport_row = row` (already
///    viewport-relative). Rects fully above/below the viewport or anchored at
///    or past the right edge are culled; a partially-above rect keeps its
///    negative row (the shader clips).
///
/// The component is (re)inserted for every terminal that has inline children
/// OR already carries `TerminalOverlays`, so a terminal whose last inline
/// child despawned converges to all-sentinel / all-`None` instead of keeping
/// stale texture handles alive.
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
        &WebviewPlacement,
        &WebviewTextureTarget,
        Has<CompositeNotified>,
        Option<&WebviewOwner>,
    )>,
    writers: Res<ConnectionWriters>,
) {
    for (terminal, grid, children, has_overlays) in &terminals {
        // NOTE: `grid.modes` refreshes only on snapshots (FrameDelta carries
        // no modes), which suffices here: alt-screen entry/exit always
        // arrives as a snapshot — alacritty's `Term::swap_alt` calls
        // `mark_fully_damaged()`, full damage collects as `DirtyRows::Full`,
        // and `decide_frame_kind` maps that to `FrameKind::Snapshot`. A
        // delta can therefore never carry an alt-screen flip past this check.
        let on_alt_screen = grid.modes.iter().any(|m| m == ALT_SCREEN_MODE);
        let mut overlays = TerminalOverlays::default();
        let mut has_webview_child = false;
        if let Some(kids) = children {
            for child in kids.iter() {
                let Ok((view, placement, texture, already_notified, owner)) = webviews.get(child)
                else {
                    continue;
                };
                has_webview_child = true;
                if (grid.last_seq.wrapping_sub(placement.frame_seq) as i32) < 0 {
                    continue;
                }
                let (viewport_row, anchor_col) = match placement.anchor {
                    AnchorMode::Scrollback { line, col } => {
                        if on_alt_screen {
                            continue;
                        }
                        let row = line as i64
                            - (grid.history_base as i64 + i64::from(grid.history_size)
                                - i64::from(grid.display_offset));
                        (row, col)
                    }
                    AnchorMode::FixedScreen { row, col } => {
                        if !on_alt_screen {
                            continue;
                        }
                        (i64::from(row), col)
                    }
                };
                if viewport_row + i64::from(placement.rows) <= 0
                    || viewport_row >= i64::from(grid.rows)
                    || u32::from(anchor_col) >= u32::from(grid.cols)
                {
                    continue;
                }
                let slot = usize::from(view.slot);
                if slot >= OVERLAY_SLOTS {
                    continue;
                }
                overlays.rects[slot] = IVec4::new(
                    viewport_row as i32,
                    i32::from(anchor_col),
                    i32::from(placement.rows),
                    i32::from(placement.cols),
                );
                overlays.textures[slot] = Some(texture.0.clone());
                if !already_notified {
                    commands.entity(child).insert(CompositeNotified);
                    if let Some(owner) = owner {
                        let msg = serde_json::to_string(&PushMsg::Compositing {
                            handle: owner.handle.clone(),
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
fn on_placement_removed(
    event: On<Remove, WebviewPlacement>,
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
        active: false,
    })
    .expect("PushMsg serializes infallibly");
    writers.send(owner.connection_id, msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webview::osc::on_osc_webview_request;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_cef::prelude::PreloadScripts;
    use ozma_tty_engine::{OscWebviewRequest, OscWebviewVerb};
    use ozma_tty_renderer::CellMetrics;

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<DynamicRegistry>()
            .init_resource::<Assets<Image>>()
            .init_resource::<ConnectionWriters>()
            .add_observer(on_osc_webview_request)
            .add_observer(on_placement_removed);
        app
    }

    fn register_dyn(app: &mut App, view_id: &str, owner_surface: Entity, interactive: bool) {
        use crate::control_plane::DynamicView;
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            view_id.into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive,
                owner_surface,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec![],
            },
        );
    }

    fn register_url(app: &mut App, view_id: &str, owner_surface: Entity, url: &str, bridge: bool) {
        use crate::control_plane::DynamicView;
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            view_id.into(),
            DynamicView {
                source: DynSource::Url {
                    url: url.into(),
                    bridge,
                },
                entry: String::new(),
                interactive: true,
                owner_surface,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec![],
            },
        );
    }

    fn spawn_terminal(app: &mut App) -> Entity {
        let surface = app.world_mut().spawn(Name::new("t")).id();
        app.world_mut().flush();
        surface
    }

    fn test_anchor() -> InlineAnchor {
        InlineAnchor {
            mode: AnchorMode::Scrollback { line: 42, col: 3 },
            frame_seq: 7,
        }
    }

    fn mount(app: &mut App, terminal: Entity, view_id: &str, anchor: Option<InlineAnchor>) {
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::Mount {
                view_id: view_id.into(),
                rows: 10,
                cols: 40,
                instance_id: None,
            },
            anchor,
        });
        app.world_mut().flush();
    }

    fn unmount(app: &mut App, terminal: Entity, view_id: Option<&str>) {
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::Unmount {
                view_id: view_id.map(str::to_string),
                instance_id: None,
            },
            anchor: None,
        });
        app.world_mut().flush();
        // NOTE: despawn is deferred; a flush + update applies it.
        app.update();
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

    fn slot_of(app: &App, terminal: Entity, view_id: &str) -> Option<u8> {
        webview_children_of(app, terminal)
            .into_iter()
            .find_map(|child| {
                app.world()
                    .get::<Webview>(child)
                    .filter(|v| v.view_id == view_id)
                    .map(|v| v.slot)
            })
    }

    fn mount_instance(
        app: &mut App,
        terminal: Entity,
        view_id: &str,
        instance_id: &str,
        anchor: Option<InlineAnchor>,
    ) {
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::Mount {
                view_id: view_id.into(),
                rows: 10,
                cols: 40,
                instance_id: Some(instance_id.into()),
            },
            anchor,
        });
        app.world_mut().flush();
    }

    fn unmount_instance(app: &mut App, terminal: Entity, view_id: &str, instance_id: &str) {
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::Unmount {
                view_id: Some(view_id.into()),
                instance_id: Some(instance_id.into()),
            },
            anchor: None,
        });
        app.world_mut().flush();
        app.update();
    }

    fn slot_of_instance(
        app: &App,
        terminal: Entity,
        view_id: &str,
        instance_id: Option<&str>,
    ) -> Option<u8> {
        webview_children_of(app, terminal)
            .into_iter()
            .find_map(|child| {
                app.world()
                    .get::<Webview>(child)
                    .filter(|v| v.view_id == view_id && v.instance_id.as_deref() == instance_id)
                    .map(|v| v.slot)
            })
    }

    #[test]
    fn mount_spawns_child_with_inline_components() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);

        mount(&mut app, terminal, "dash", Some(test_anchor()));

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
                view_id: "dash".into(),
                instance_id: None,
                slot: 0
            }),
        );
        assert_eq!(
            app.world().get::<WebviewPlacement>(child),
            Some(&WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7
            }),
        );
        match app
            .world()
            .get::<WebviewSource>(child)
            .expect("webview must carry WebviewSource")
        {
            WebviewSource::Url(url) => assert_eq!(url, "ozma-dyn://dash/index.html"),
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
            "an inline (bridged) webview must carry the populated window.ozma preload"
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

    #[test]
    fn duplicate_mount_updates_placement_in_place() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);

        mount(&mut app, terminal, "dash", Some(test_anchor()));
        let before = webview_children_of(&app, terminal);
        assert_eq!(before.len(), 1, "first mount spawns one child");
        let entity = before[0];
        let slot_before = app.world().get::<Webview>(entity).unwrap().slot;

        // Re-mount the same handle with a different anchor.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::Mount {
                view_id: "dash".into(),
                rows: 12,
                cols: 50,
                instance_id: None,
            },
            anchor: Some(InlineAnchor {
                mode: AnchorMode::Scrollback { line: 99, col: 7 },
                frame_seq: 9,
            }),
        });
        app.world_mut().flush();

        let after = webview_children_of(&app, terminal);
        assert_eq!(after.len(), 1, "re-mount must NOT spawn a second child");
        assert_eq!(
            after[0], entity,
            "re-mount must reuse the same entity (no reload)"
        );
        assert_eq!(
            app.world().get::<WebviewPlacement>(entity),
            Some(&WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 99, col: 7 },
                rows: 12,
                cols: 50,
                frame_seq: 9,
            }),
            "re-mount updates the placement in place"
        );
        assert_eq!(
            app.world().get::<Webview>(entity).unwrap().slot,
            slot_before,
            "re-mount preserves the overlay slot"
        );
    }

    #[test]
    fn slots_fill_in_order_and_a_fifth_mount_is_rejected() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        for id in ["a", "b", "c", "d", "e"] {
            register_dyn(&mut app, id, terminal, true);
        }

        for id in ["a", "b", "c", "d"] {
            mount(&mut app, terminal, id, Some(test_anchor()));
        }
        assert_eq!(slot_of(&app, terminal, "a"), Some(0));
        assert_eq!(slot_of(&app, terminal, "b"), Some(1));
        assert_eq!(slot_of(&app, terminal, "c"), Some(2));
        assert_eq!(slot_of(&app, terminal, "d"), Some(3));

        mount(&mut app, terminal, "e", Some(test_anchor()));
        assert_eq!(
            webview_children_of(&app, terminal).len(),
            OVERLAY_SLOTS,
            "a fifth mount must be rejected once all slots are taken"
        );
        assert_eq!(slot_of(&app, terminal, "e"), None);
    }

    #[test]
    fn unmount_frees_the_slot_for_the_next_mount() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        for id in ["a", "b", "c"] {
            register_dyn(&mut app, id, terminal, true);
        }

        mount(&mut app, terminal, "a", Some(test_anchor()));
        mount(&mut app, terminal, "b", Some(test_anchor()));
        unmount(&mut app, terminal, Some("a"));
        assert_eq!(
            webview_children_of(&app, terminal).len(),
            1,
            "unmounting one view must despawn exactly its child"
        );

        mount(&mut app, terminal, "c", Some(test_anchor()));
        assert_eq!(
            slot_of(&app, terminal, "c"),
            Some(0),
            "the freed slot 0 must be reused by the next mount"
        );
        assert_eq!(slot_of(&app, terminal, "b"), Some(1));
    }

    #[test]
    fn unmount_all_despawns_every_inline_child() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        for id in ["a", "b"] {
            register_dyn(&mut app, id, terminal, true);
        }
        mount(&mut app, terminal, "a", Some(test_anchor()));
        mount(&mut app, terminal, "b", Some(test_anchor()));
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
        register_dyn(&mut app, "hud", terminal, false);

        mount(&mut app, terminal, "hud", Some(test_anchor()));

        let children = webview_children_of(&app, terminal);
        assert_eq!(children.len(), 1);
        assert!(
            app.world().get::<NonInteractive>(children[0]).is_some(),
            "a non-interactive view must carry NonInteractive"
        );
    }

    #[test]
    fn mount_without_anchor_is_dropped() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);

        mount(&mut app, terminal, "dash", None);

        assert!(
            webview_children_of(&app, terminal).is_empty(),
            "a mount without an anchor must be dropped"
        );
    }

    #[test]
    fn mount_of_unregistered_view_is_dropped() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "ghost", Some(test_anchor()));

        assert!(
            webview_children_of(&app, terminal).is_empty(),
            "a mount for an unregistered view must be dropped"
        );
    }

    #[test]
    fn two_instances_of_same_view_both_mount_in_separate_slots() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "memo", terminal, true);

        mount_instance(&mut app, terminal, "memo", "a", Some(test_anchor()));
        mount_instance(&mut app, terminal, "memo", "b", Some(test_anchor()));

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            2,
            "two distinct (view_id, instance_id) tuples must both mount"
        );
        assert_eq!(slot_of_instance(&app, terminal, "memo", Some("a")), Some(0));
        assert_eq!(slot_of_instance(&app, terminal, "memo", Some("b")), Some(1));
    }

    #[test]
    fn duplicate_view_instance_tuple_is_rejected() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "memo", terminal, true);

        mount_instance(&mut app, terminal, "memo", "a", Some(test_anchor()));
        mount_instance(&mut app, terminal, "memo", "a", Some(test_anchor()));

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            1,
            "a duplicate (view_id, instance_id) mount must be dropped"
        );
    }

    #[test]
    fn default_instance_and_named_instance_coexist() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "memo", terminal, true);

        mount(&mut app, terminal, "memo", Some(test_anchor()));
        mount_instance(&mut app, terminal, "memo", "a", Some(test_anchor()));

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            2,
            "the default (None) instance and a named instance are distinct"
        );
        assert_eq!(slot_of_instance(&app, terminal, "memo", None), Some(0));
        assert_eq!(slot_of_instance(&app, terminal, "memo", Some("a")), Some(1));
    }

    #[test]
    fn unmount_one_instance_leaves_the_other() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "memo", terminal, true);

        mount_instance(&mut app, terminal, "memo", "a", Some(test_anchor()));
        mount_instance(&mut app, terminal, "memo", "b", Some(test_anchor()));

        unmount_instance(&mut app, terminal, "memo", "a");

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            1,
            "unmounting one instance must despawn exactly that instance"
        );
        assert_eq!(slot_of_instance(&app, terminal, "memo", Some("a")), None);
        assert_eq!(slot_of_instance(&app, terminal, "memo", Some("b")), Some(1));
    }

    #[test]
    fn unmount_view_scope_despawns_every_instance_of_that_view() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "memo", terminal, true);
        register_dyn(&mut app, "other", terminal, true);

        mount_instance(&mut app, terminal, "memo", "a", Some(test_anchor()));
        mount_instance(&mut app, terminal, "memo", "b", Some(test_anchor()));
        mount(&mut app, terminal, "other", Some(test_anchor()));

        unmount(&mut app, terminal, Some("memo"));

        assert_eq!(
            webview_children_of(&app, terminal).len(),
            1,
            "view-scoped unmount must despawn every instance of that view_id only"
        );
        assert_eq!(slot_of_instance(&app, terminal, "other", None), Some(2));
    }

    #[test]
    fn slot_cap_counts_all_instances_together() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "memo", terminal, true);

        for inst in ["a", "b", "c", "d"] {
            mount_instance(&mut app, terminal, "memo", inst, Some(test_anchor()));
        }
        assert_eq!(webview_children_of(&app, terminal).len(), OVERLAY_SLOTS);

        mount_instance(&mut app, terminal, "memo", "e", Some(test_anchor()));
        assert_eq!(
            webview_children_of(&app, terminal).len(),
            OVERLAY_SLOTS,
            "the 4-slot cap is per-terminal across all instances; a 5th is rejected"
        );
        assert_eq!(slot_of_instance(&app, terminal, "memo", Some("e")), None);
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

    fn projection_grid(last_seq: u32) -> TerminalGrid {
        TerminalGrid {
            cols: 80,
            rows: 24,
            last_seq,
            history_base: 0,
            history_size: 40,
            display_offset: 0,
            ..Default::default()
        }
    }

    fn spawn_projection_child(
        app: &mut App,
        terminal: Entity,
        slot: u8,
        placement: WebviewPlacement,
    ) -> Handle<Image> {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.world_mut().spawn((
            ChildOf(terminal),
            Webview {
                view_id: format!("view-{slot}"),
                instance_id: None,
                slot,
            },
            placement,
            WebviewTextureTarget(handle.clone()),
        ));
        handle
    }

    fn project(app: &mut App) {
        app.world_mut()
            .run_system_once(project_webview_overlays)
            .unwrap();
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

    #[test]
    fn projection_holds_until_grid_seq_reaches_anchor_seq() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(6)).id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));

        app.world_mut()
            .get_mut::<TerminalGrid>(terminal)
            .unwrap()
            .last_seq = 7;
        project(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_eq!(
            overlays.rects[0],
            IVec4::new(2, 3, 10, 40),
            "once last_seq reaches frame_seq the rect must project (42 - 40 = row 2)"
        );
        assert!(overlays.textures[0].is_some());
    }

    fn formula_grid(history_size: u32, display_offset: u32) -> TerminalGrid {
        TerminalGrid {
            cols: 80,
            rows: 5,
            last_seq: 1,
            history_base: 0,
            history_size,
            display_offset,
            ..Default::default()
        }
    }

    fn formula_placement() -> WebviewPlacement {
        WebviewPlacement {
            anchor: AnchorMode::Scrollback { line: 30, col: 2 },
            rows: 4,
            cols: 10,
            frame_seq: 0,
        }
    }

    #[test]
    fn projection_formula_maps_absolute_line_to_viewport_row() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(formula_grid(26, 0)).id();
        spawn_projection_child(&mut app, terminal, 0, formula_placement());

        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(4, 2, 4, 10),
            "30 - (0 + 26 - 0) must land on viewport row 4"
        );
    }

    #[test]
    fn projection_formula_culls_rect_pushed_below_viewport_by_scrollback() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(formula_grid(26, 3)).id();
        spawn_projection_child(&mut app, terminal, 0, formula_placement());

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));
    }

    #[test]
    fn projection_formula_keeps_negative_row_for_partially_above_rect() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(formula_grid(32, 0)).id();
        spawn_projection_child(&mut app, terminal, 0, formula_placement());

        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(-2, 2, 4, 10),
            "a partially-above rect must pass its negative row through as i32"
        );
    }

    #[test]
    fn projection_culls_fully_outside_rects() {
        let mut app = make_test_app();
        let terminal = app
            .world_mut()
            .spawn(TerminalGrid {
                cols: 80,
                rows: 24,
                last_seq: 1,
                history_base: 0,
                history_size: 30,
                display_offset: 0,
                ..Default::default()
            })
            .id();
        let above = WebviewPlacement {
            anchor: AnchorMode::Scrollback { line: 10, col: 0 },
            rows: 10,
            cols: 10,
            frame_seq: 0,
        };
        let below = WebviewPlacement {
            anchor: AnchorMode::Scrollback { line: 100, col: 0 },
            rows: 10,
            cols: 10,
            frame_seq: 0,
        };
        let col_out = WebviewPlacement {
            anchor: AnchorMode::Scrollback { line: 35, col: 80 },
            rows: 10,
            cols: 10,
            frame_seq: 0,
        };
        spawn_projection_child(&mut app, terminal, 0, above);
        spawn_projection_child(&mut app, terminal, 1, below);
        spawn_projection_child(&mut app, terminal, 2, col_out);

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));
    }

    #[test]
    fn projection_keeps_rect_anchored_at_last_valid_column() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 79 },
                rows: 10,
                cols: 10,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(2, 79, 10, 10),
            "a rect anchored at the last valid column (cols - 1) must project, not cull"
        );
    }

    #[test]
    fn alt_screen_blanks_all_slots() {
        let mut app = make_test_app();
        let terminal = app
            .world_mut()
            .spawn(TerminalGrid {
                modes: vec![ALT_SCREEN_MODE.to_string()],
                ..projection_grid(7)
            })
            .id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));

        app.world_mut()
            .get_mut::<TerminalGrid>(terminal)
            .unwrap()
            .modes
            .clear();
        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(2, 3, 10, 40),
            "returning to the primary screen must re-project the placement"
        );
    }

    #[test]
    fn fixed_screen_projects_to_its_row_on_alt_screen() {
        let mut app = make_test_app();
        let terminal = app
            .world_mut()
            .spawn(TerminalGrid {
                modes: vec![ALT_SCREEN_MODE.to_string()],
                ..projection_grid(7)
            })
            .id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::FixedScreen { row: 5, col: 2 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(5, 2, 4, 10),
            "a FixedScreen placement projects to its own viewport row on the alt screen"
        );
    }

    #[test]
    fn fixed_screen_is_hidden_on_primary_screen() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::FixedScreen { row: 5, col: 2 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));
    }

    #[test]
    fn texture_handle_lands_in_the_childs_slot() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        let handle = spawn_projection_child(
            &mut app,
            terminal,
            2,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
        );

        project(&mut app);
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

    #[test]
    fn projection_draws_two_instances_in_their_own_slots() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        let placement = WebviewPlacement {
            anchor: AnchorMode::Scrollback { line: 42, col: 3 },
            rows: 10,
            cols: 40,
            frame_seq: 7,
        };
        let h0 = spawn_projection_child(&mut app, terminal, 0, placement);
        let h1 = spawn_projection_child(&mut app, terminal, 1, placement);

        project(&mut app);
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

    #[test]
    fn stale_overlays_clear_after_unmount_all() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);
        app.world_mut()
            .entity_mut(terminal)
            .insert(projection_grid(7));

        mount(&mut app, terminal, "dash", Some(test_anchor()));
        project(&mut app);
        let overlays = overlays_of(&app, terminal);
        assert_ne!(overlays.rects[0], IVec4::ZERO);
        assert!(overlays.textures[0].is_some());

        unmount(&mut app, terminal, None);
        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));
    }

    #[test]
    fn seq_hold_is_wrap_aware_near_u32_max() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(u32::MAX - 3)).id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: u32::MAX - 1,
            },
        );

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));

        app.world_mut()
            .get_mut::<TerminalGrid>(terminal)
            .unwrap()
            .last_seq = 2;
        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(2, 3, 10, 40),
            "a last_seq that wrapped past 0 must release the hold"
        );
    }

    #[test]
    fn size_sync_updates_webview_size_when_metrics_change() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);
        mount(&mut app, terminal, "dash", Some(test_anchor()));
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
        let mut app = make_test_app();
        app.init_resource::<SizeChangeProbe>();
        app.add_systems(
            Update,
            (sync_webview_size, probe_webview_size_changed).chain(),
        );
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);
        mount(&mut app, terminal, "dash", Some(test_anchor()));

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
                    view_id: format!("view-{slot}"),
                    instance_id: None,
                    slot,
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

    fn register_dynamic_dir(app: &mut App, handle: &str, owner_surface: Entity) {
        use crate::control_plane::DynamicView;
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            handle.into(),
            DynamicView {
                source: DynSource::Dir("/abs/ui".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec![],
            },
        );
    }

    #[test]
    fn mount_of_dynamic_handle_uses_ozmux_dyn_url_and_no_bridge() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dynamic_dir(&mut app, "DYN1", terminal);

        mount(&mut app, terminal, "DYN1", Some(test_anchor()));

        let children = webview_children_of(&app, terminal);
        assert_eq!(
            children.len(),
            1,
            "dynamic mount must spawn one inline child"
        );
        match app.world().get::<WebviewSource>(children[0]).unwrap() {
            WebviewSource::Url(u) => assert_eq!(u, "ozma-dyn://DYN1/index.html"),
            other => panic!("expected ozma-dyn URL, got {other:?}"),
        }
        let preload = app.world().get::<PreloadScripts>(children[0]).unwrap();
        assert!(
            !preload.0.iter().any(|s| s.contains("__ozmuxGranted")),
            "a dynamic view must carry no capability grant / host bridge"
        );
    }

    #[test]
    fn resolve_mount_enforces_owner_surface() {
        use crate::control_plane::{DynSource, DynamicRegistry, DynamicView};
        let owner = Entity::from_bits(1);
        let other = Entity::from_bits(2);
        let mut dynamic = DynamicRegistry::default();
        dynamic.insert(
            "DYNHANDLE".into(),
            DynamicView {
                source: DynSource::Dir("/abs/ui".into()),
                entry: "index.html".into(),
                interactive: false,
                owner_surface: owner,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec![],
            },
        );

        let d = resolve_mount("DYNHANDLE", owner, &dynamic).expect("dynamic resolves");
        assert_eq!(d.url.as_deref(), Some("ozma-dyn://DYNHANDLE/index.html"));
        assert!(!d.interactive);

        assert!(
            resolve_mount("DYNHANDLE", other, &dynamic).is_none(),
            "a handle resolves only from its owner surface"
        );
        assert!(resolve_mount("ghost", owner, &dynamic).is_none());
    }

    #[test]
    fn resolve_mount_dynamic_inline_yields_ozmux_dyn_url_via_index_html() {
        use crate::control_plane::{DynSource, DynamicRegistry, DynamicView};
        let owner = Entity::from_bits(1);
        let mut dynamic = DynamicRegistry::default();
        dynamic.insert(
            "INLINEH".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: owner,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec![],
            },
        );
        let r = resolve_mount("INLINEH", owner, &dynamic).expect("inline resolves");
        assert_eq!(r.url.as_deref(), Some("ozma-dyn://INLINEH/index.html"));
        assert!(r.owner.is_some());
    }

    #[test]
    fn dynamic_mount_stamps_webview_owner() {
        use crate::control_plane::{DynSource, DynamicView, WebviewOwner};
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            "HANDLE".into(),
            DynamicView {
                source: DynSource::Inline("<h1>hi</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: terminal,
                connection_id: 42,
                forward_keys: vec![],
                preload: vec![],
            },
        );

        mount(&mut app, terminal, "HANDLE", Some(test_anchor()));

        let children = webview_children_of(&app, terminal);
        assert_eq!(
            children.len(),
            1,
            "dynamic mount must spawn one inline child"
        );
        let child = children[0];
        let owner = app
            .world()
            .get::<WebviewOwner>(child)
            .expect("dynamic mount must stamp WebviewOwner");
        assert_eq!(owner.connection_id, 42);
        assert_eq!(owner.handle, "HANDLE");
    }

    #[test]
    fn alt_screen_exit_despawns_only_fixed_screen_children() {
        use ozma_tty_engine::TerminalModeChanged;

        let mut app = make_test_app();
        app.add_observer(despawn_fixed_screen_on_alt_exit);
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::FixedScreen { row: 1, col: 0 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );
        spawn_projection_child(
            &mut app,
            terminal,
            1,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 0 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );
        let fixed_entity = webview_children_of(&app, terminal)
            .into_iter()
            .find(|e| {
                matches!(
                    app.world().get::<WebviewPlacement>(*e).unwrap().anchor,
                    AnchorMode::FixedScreen { .. }
                )
            })
            .unwrap();

        app.world_mut().trigger(TerminalModeChanged {
            entity: terminal,
            added: vec![],
            removed: vec![ALT_SCREEN_MODE.to_string()],
        });
        app.world_mut().flush();
        app.update();

        let remaining = webview_children_of(&app, terminal);
        assert_eq!(
            remaining.len(),
            1,
            "the FixedScreen child must be despawned"
        );
        assert!(
            !remaining.contains(&fixed_entity),
            "the despawned child must be the FixedScreen one"
        );
        assert!(matches!(
            app.world()
                .get::<WebviewPlacement>(remaining[0])
                .unwrap()
                .anchor,
            AnchorMode::Scrollback { .. }
        ));
    }

    #[test]
    fn resolve_mount_url_returns_verbatim_url_and_gates_owner_on_bridge() {
        use crate::control_plane::{DynSource, DynamicView};
        let surface = Entity::from_bits(1);
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "disp".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://example.com".into(),
                    bridge: false,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 7,
                forward_keys: vec![],
                preload: vec![],
            },
        );
        reg.insert(
            "appv".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://app.example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 7,
                forward_keys: vec![],
                preload: vec![],
            },
        );

        let disp = resolve_mount("disp", surface, &reg).expect("registered");
        assert_eq!(disp.url.as_deref(), Some("https://example.com"));
        assert!(disp.owner.is_none(), "display-only url must have no owner");

        let appv = resolve_mount("appv", surface, &reg).expect("registered");
        assert_eq!(appv.url.as_deref(), Some("https://app.example.com"));
        assert_eq!(appv.owner, Some((7, "appv".to_string())));
    }

    #[test]
    fn mount_url_display_only_has_no_preload_or_owner() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_url(&mut app, "disp", terminal, "https://example.com", false);

        mount(&mut app, terminal, "disp", Some(test_anchor()));

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
        // the ozmux bridge scripts are absent (empty vec), not that the component
        // itself is absent.
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts always present via WebviewSource #[require]");
        assert!(
            preload.0.is_empty(),
            "a display-only url must carry no ozmux bridge scripts"
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
        register_url(&mut app, "appv", terminal, "https://app.example.com", true);

        mount(&mut app, terminal, "appv", Some(test_anchor()));

        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert!(
            !preload.0.is_empty(),
            "a bridged url must carry the ozmux bridge scripts"
        );
        assert_eq!(
            app.world().get::<WebviewOwner>(child),
            Some(&WebviewOwner {
                connection_id: 1,
                handle: "appv".into(),
            }),
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
        placement: WebviewPlacement,
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
                    view_id: format!("view-{slot}"),
                    instance_id: None,
                    slot,
                },
                placement,
                WebviewTextureTarget(image_handle.clone()),
                WebviewOwner {
                    connection_id,
                    handle: handle.to_string(),
                },
            ))
            .id()
    }

    #[test]
    fn first_projection_sends_compositing_start() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        let entity = spawn_owned_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
            1,
            "myhandle",
        );

        project(&mut app);

        assert!(
            app.world().get::<CompositeNotified>(entity).is_some(),
            "first successful projection must stamp CompositeNotified"
        );
        let msg = rx
            .try_recv()
            .expect("compositing start must be sent after first projection");
        assert_eq!(
            msg,
            r#"{"op":"compositing","handle":"myhandle","active":true}"#
        );
    }

    #[test]
    fn second_projection_does_not_resend() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        spawn_owned_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
            1,
            "myhandle",
        );

        project(&mut app);
        let _ = rx.try_recv().expect("first projection must send start");

        project(&mut app);
        assert!(
            rx.try_recv().is_err(),
            "second projection must NOT send a duplicate start"
        );
    }

    #[test]
    fn stop_observer_sends_compositing_stop_when_notified() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        let child = spawn_owned_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
            1,
            "myhandle",
        );

        project(&mut app);
        let _ = rx.try_recv().expect("start notification must arrive");

        app.world_mut().entity_mut(child).despawn();
        app.world_mut().flush();

        let msg = rx
            .try_recv()
            .expect("compositing stop must be sent on despawn");
        assert_eq!(
            msg,
            r#"{"op":"compositing","handle":"myhandle","active":false}"#
        );
    }

    #[test]
    fn stop_observer_does_not_send_when_not_notified() {
        let mut app = make_test_app();
        let (writers, rx) = compositing_writers(1);
        app.insert_resource(writers);
        let terminal = app.world_mut().spawn(projection_grid(6)).id();
        let child = spawn_owned_projection_child(
            &mut app,
            terminal,
            0,
            WebviewPlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 3 },
                rows: 10,
                cols: 40,
                frame_seq: 7,
            },
            1,
            "myhandle",
        );

        app.world_mut().entity_mut(child).despawn();
        app.world_mut().flush();

        assert!(
            rx.try_recv().is_err(),
            "despawning a never-projected entity must NOT send a stop notification"
        );
    }

    #[test]
    fn mount_bridged_inline_appends_user_preload_after_bridge() {
        use crate::control_plane::{DynSource, DynamicView};
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            "h".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: terminal,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec!["window.USER = 1;".into()],
            },
        );

        mount(&mut app, terminal, "h", Some(test_anchor()));

        let child = webview_children_of(&app, terminal)[0];
        let preload = app
            .world()
            .get::<PreloadScripts>(child)
            .expect("PreloadScripts present");
        assert!(preload.0.len() >= 2, "bridge + user script");
        assert_eq!(
            preload.0.last().map(String::as_str),
            Some("window.USER = 1;"),
            "the user script must come last, after the ozma bridge"
        );
    }

    #[test]
    fn mount_bridged_url_appends_user_preload_after_bridge() {
        use crate::control_plane::{DynSource, DynamicView};
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            "u".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://app.example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: terminal,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec!["window.USER = 1;".into()],
            },
        );

        mount(&mut app, terminal, "u", Some(test_anchor()));

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
        use crate::control_plane::{DynSource, DynamicView, WebviewOwner};
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            "disp".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://example.com".into(),
                    bridge: false,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: terminal,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec!["window.USER = 1;".into()],
            },
        );

        mount(&mut app, terminal, "disp", Some(test_anchor()));

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
}
