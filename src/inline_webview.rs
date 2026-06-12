//! Inline webviews: `ChildOf` children of a terminal surface that render a
//! registered view into the terminal's text flow. This module owns the
//! components and the mount/unmount policy executed by the `MountInline` /
//! `UnmountInline` arms of `osc_webview::on_osc_webview_request`.

use crate::extension_render::preload::{build_preload, webview_url};
use crate::osc_webview::{GrantedNamespaces, NonInteractive};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::{WebviewSize, WebviewSource, WebviewTextureTarget};
use bevy_terminal::InlineAnchor;
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::prelude::OVERLAY_SLOTS;
use ozmux_extension_host::ViewRegistry;

// TODO: Task 5 adds the per-frame projection / size-sync systems and their
// plugin. Task 3 registers nothing — the mount/unmount arms ride the existing
// `on_osc_webview_request` observer wired by `OzmuxOscWebviewPlugin`.

/// Marks an inline webview entity and records its identity: the mounted
/// `view_id` and the overlay texture `slot` (0..`OVERLAY_SLOTS`) it occupies
/// on its parent terminal. The owning terminal surface is NOT duplicated
/// here — it is the `ChildOf` parent, per the multiplexer's "no typed
/// back-references" convention. Each child's `slot` is the single source of
/// truth for slot allocation (no separate allocation table).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineWebview {
    /// The registered view id this inline webview was mounted from.
    pub(crate) view_id: String,
    /// The overlay texture slot (0..`OVERLAY_SLOTS`) on the parent terminal.
    pub(crate) slot: u8,
}

/// Where an inline webview sits in its terminal's scrollback: the anchor cell
/// (absolute line = `history_base + history_size + cursor row` at the OSC byte
/// position, plus the cursor column), the rect extent in cells, and the VT
/// `frame_seq` the next grid emit carries (Task 5 defers first projection
/// until the grid catches up to it).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlinePlacement {
    /// Absolute scrollback line of the rect's TOP row.
    pub(crate) anchor_line: u64,
    /// Column of the anchor cell.
    pub(crate) anchor_col: u16,
    /// Rect height in terminal cells.
    pub(crate) rows: u16,
    /// Rect width in terminal cells.
    pub(crate) cols: u16,
    /// The VT frame seq stamped at mount; grid frames at or after this seq
    /// (wrap-aware compare) may project the placement.
    pub(crate) frame_seq: u32,
}

/// Everything the `MountInline` verb carries into `mount_inline`: the target
/// terminal surface, its resolved multiplexer owners (for the preload
/// context), and the parsed verb + anchor payload.
pub(crate) struct InlineMountContext<'a> {
    /// The requesting terminal surface — the `ChildOf` parent of the mount.
    pub(crate) terminal_surface: Entity,
    /// The workspace owning the terminal's pane (preload context).
    pub(crate) workspace: Entity,
    /// The pane owning the terminal surface (preload context).
    pub(crate) pane: Entity,
    /// The registered view id to mount.
    pub(crate) view_id: &'a str,
    /// Rect height in terminal cells (validated 1..=200 by `bevy_terminal`).
    pub(crate) rows: u16,
    /// Rect width in terminal cells (validated 1..=400 by `bevy_terminal`).
    pub(crate) cols: u16,
    /// The VT-stamped anchor; `None` is a policy rejection (gate 1).
    pub(crate) anchor: Option<InlineAnchor>,
}

/// The system params `mount_inline` / `unmount_inline` need, bundled so the
/// `on_osc_webview_request` observer gains a single extra parameter.
#[derive(SystemParam)]
pub(crate) struct InlineWebviewParams<'w, 's> {
    commands: Commands<'w, 's>,
    images: ResMut<'w, Assets<Image>>,
    children: Query<'w, 's, &'static Children>,
    views: Query<'w, 's, &'static InlineWebview>,
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

/// Mounts a registered view as an inline webview child of the requesting
/// terminal surface, applying the policy gates in order (each rejection is a
/// `tracing::debug!` + return): missing anchor, unregistered view, duplicate
/// `view_id` on this terminal, overlay-slot exhaustion.
///
/// The parent (`ctx.terminal_surface`, the `OscWebviewRequest` target) is the
/// multiplexer Surface entity itself: `finish_terminal_setup` inserts both
/// `TerminalBundle` (`TerminalHandle`, which emits the OSC request) and
/// `TerminalRenderBundle` (`TerminalGrid`) onto that one entity, so the
/// `ChildOf` parent is also the entity Task 5's projection reads grid state
/// from.
///
/// `WebviewSize` is seeded here because `bevy_cef` builds the CEF browser
/// from it at creation. The seed is `(cols × cell_w, rows × cell_h) /
/// scale_factor` in logical px from `TerminalCellMetricsResource` and the
/// primary window; when neither exists yet (headless tests, pre-first-render)
/// a placeholder cell of 8×16 physical px at scale 1.0 is used — Task 5's
/// size-sync system corrects it.
pub(crate) fn mount_inline(
    params: &mut InlineWebviewParams,
    registry: &ViewRegistry,
    ctx: InlineMountContext<'_>,
) {
    let Some(anchor) = ctx.anchor else {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: mount-inline without anchor, dropping");
        return;
    };
    let Some(view) = registry.get(ctx.view_id) else {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: mount-inline for unregistered view, dropping");
        return;
    };
    let live = live_inline_children(&params.children, &params.views, ctx.terminal_surface);
    if live.iter().any(|(_, v)| v.view_id == ctx.view_id) {
        tracing::debug!(view_id = %ctx.view_id, "osc-webview: duplicate inline mount on this terminal, dropping");
        return;
    }
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
    let granted = GrantedNamespaces(view.capabilities.iter().cloned().collect());
    let webview = params.commands.spawn_empty().id();
    let preload = build_preload(ctx.workspace, ctx.pane, webview, &view.owning_ext, &granted);
    // NOTE: keep this entity free of Node / Mesh2d / Mesh3d / Sprite /
    // MaterialNode (even for debug visualization). bevy_cef's mesh/sprite
    // input paths and display-size allocators key on `With<WebviewSource>`
    // plus exactly those components; adding one double-attaches input
    // forwarding and display allocation on top of ozmux's inline routing
    // (design spec §4 invariant).
    params.commands.entity(webview).insert((
        ChildOf(ctx.terminal_surface),
        WebviewSource::new(webview_url(&view.owning_ext, &view.entry)),
        texture,
        WebviewSize(size),
        InlineWebview {
            view_id: ctx.view_id.to_string(),
            slot,
        },
        InlinePlacement {
            anchor_line: anchor.line,
            anchor_col: anchor.col,
            rows: ctx.rows,
            cols: ctx.cols,
            frame_seq: anchor.frame_seq,
        },
        granted,
        preload,
    ));
    if !view.interactive {
        params.commands.entity(webview).insert(NonInteractive);
    }
    tracing::debug!(
        view_id = %ctx.view_id,
        terminal = ?ctx.terminal_surface,
        slot,
        rows = ctx.rows,
        cols = ctx.cols,
        anchor_line = anchor.line,
        "osc-webview: inline webview mounted"
    );
}

/// Despawns the inline child(ren) of `terminal_surface` matching `view_id`,
/// or ALL of them when `view_id` is `None`. The all-case is an explicit
/// iteration over every live inline child — it also executes the
/// VT-synthesized fold/saturation `UnmountInline { view_id: None }` frames.
pub(crate) fn unmount_inline(
    params: &mut InlineWebviewParams,
    terminal_surface: Entity,
    view_id: Option<&str>,
) {
    let targets: Vec<Entity> =
        live_inline_children(&params.children, &params.views, terminal_surface)
            .into_iter()
            .filter(|(_, v)| view_id.is_none_or(|id| v.view_id == id))
            .map(|(entity, _)| entity)
            .collect();
    for entity in targets {
        params.commands.entity(entity).despawn();
    }
}

const FALLBACK_CELL_W_PHYS: f32 = 8.0;
const FALLBACK_CELL_H_PHYS: f32 = 16.0;

/// The live inline-webview children of a terminal surface.
fn live_inline_children<'a>(
    children: &Query<&Children>,
    views: &'a Query<&'static InlineWebview>,
    terminal_surface: Entity,
) -> Vec<(Entity, &'a InlineWebview)> {
    let Ok(kids) = children.get(terminal_surface) else {
        return Vec::new();
    };
    kids.iter()
        .filter_map(|child| views.get(child).ok().map(|view| (child, view)))
        .collect()
}

/// The smallest slot in `0..OVERLAY_SLOTS` not occupied by a live child, or
/// `None` when every slot is taken.
fn smallest_free_slot(live: &[(Entity, &InlineWebview)]) -> Option<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc_webview::on_osc_webview_request;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_cef::prelude::PreloadScripts;
    use bevy_terminal::{OscWebviewRequest, OscWebviewVerb};
    use ozmux_extension_host::RegisteredView;
    use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin};

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .init_resource::<ViewRegistry>()
            .init_resource::<Assets<Image>>()
            .add_observer(on_osc_webview_request);
        app
    }

    fn register_view(app: &mut App, view_id: &str, interactive: bool, caps: &[&str]) {
        app.world_mut().resource_mut::<ViewRegistry>().register(
            view_id.into(),
            RegisteredView {
                entry: "ui/dash.html".into(),
                owning_ext: "memo".into(),
                interactive,
                capabilities: caps.iter().map(|s| (*s).to_string()).collect(),
            },
        );
    }

    fn spawn_terminal(app: &mut App) -> Entity {
        let surface = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("t".into())).surface
            })
            .unwrap();
        app.world_mut().flush();
        surface
    }

    fn test_anchor() -> InlineAnchor {
        InlineAnchor {
            line: 42,
            col: 3,
            frame_seq: 7,
        }
    }

    fn mount(app: &mut App, terminal: Entity, view_id: &str, anchor: Option<InlineAnchor>) {
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::MountInline {
                view_id: view_id.into(),
                rows: 10,
                cols: 40,
            },
            anchor,
        });
        app.world_mut().flush();
    }

    fn unmount(app: &mut App, terminal: Entity, view_id: Option<&str>) {
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::UnmountInline {
                view_id: view_id.map(str::to_string),
            },
            anchor: None,
        });
        app.world_mut().flush();
        // Despawn is deferred; an update applies it.
        app.update();
    }

    fn inline_children_of(app: &App, terminal: Entity) -> Vec<Entity> {
        let world = app.world();
        world
            .get::<Children>(terminal)
            .map(|children| {
                children
                    .iter()
                    .filter(|child| world.get::<InlineWebview>(*child).is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn slot_of(app: &App, terminal: Entity, view_id: &str) -> Option<u8> {
        inline_children_of(app, terminal)
            .into_iter()
            .find_map(|child| {
                app.world()
                    .get::<InlineWebview>(child)
                    .filter(|v| v.view_id == view_id)
                    .map(|v| v.slot)
            })
    }

    #[test]
    fn mount_spawns_child_with_inline_components() {
        let mut app = make_test_app();
        register_view(&mut app, "dash", true, &["fs"]);
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "dash", Some(test_anchor()));

        let children = inline_children_of(&app, terminal);
        assert_eq!(children.len(), 1, "mount must spawn one inline child");
        let child = children[0];

        assert_eq!(
            app.world().get::<ChildOf>(child).map(|c| c.parent()),
            Some(terminal),
            "the inline webview must be a ChildOf the terminal surface"
        );
        assert_eq!(
            app.world().get::<InlineWebview>(child),
            Some(&InlineWebview {
                view_id: "dash".into(),
                slot: 0
            }),
        );
        assert_eq!(
            app.world().get::<InlinePlacement>(child),
            Some(&InlinePlacement {
                anchor_line: 42,
                anchor_col: 3,
                rows: 10,
                cols: 40,
                frame_seq: 7
            }),
        );
        match app
            .world()
            .get::<WebviewSource>(child)
            .expect("inline webview must carry WebviewSource")
        {
            WebviewSource::Url(url) => assert_eq!(url, "ozmux-ext://memo/ui/dash.html"),
            other => panic!("unexpected WebviewSource: {other:?}"),
        }
        assert!(
            app.world().get::<WebviewTextureTarget>(child).is_some(),
            "inline webview must carry a headless WebviewTextureTarget"
        );
        let granted = app
            .world()
            .get::<GrantedNamespaces>(child)
            .expect("inline webview must carry GrantedNamespaces");
        assert!(granted.0.contains("fs"));
        assert!(
            app.world().get::<PreloadScripts>(child).is_some(),
            "inline webview must carry the shared preload scripts"
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
    fn duplicate_mount_same_view_is_rejected() {
        let mut app = make_test_app();
        register_view(&mut app, "dash", true, &[]);
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "dash", Some(test_anchor()));
        mount(&mut app, terminal, "dash", Some(test_anchor()));

        assert_eq!(
            inline_children_of(&app, terminal).len(),
            1,
            "a duplicate (terminal, view_id) mount must be dropped"
        );
    }

    #[test]
    fn slots_fill_in_order_and_a_fifth_mount_is_rejected() {
        let mut app = make_test_app();
        for id in ["a", "b", "c", "d", "e"] {
            register_view(&mut app, id, true, &[]);
        }
        let terminal = spawn_terminal(&mut app);

        for id in ["a", "b", "c", "d"] {
            mount(&mut app, terminal, id, Some(test_anchor()));
        }
        assert_eq!(slot_of(&app, terminal, "a"), Some(0));
        assert_eq!(slot_of(&app, terminal, "b"), Some(1));
        assert_eq!(slot_of(&app, terminal, "c"), Some(2));
        assert_eq!(slot_of(&app, terminal, "d"), Some(3));

        mount(&mut app, terminal, "e", Some(test_anchor()));
        assert_eq!(
            inline_children_of(&app, terminal).len(),
            OVERLAY_SLOTS,
            "a fifth mount must be rejected once all slots are taken"
        );
        assert_eq!(slot_of(&app, terminal, "e"), None);
    }

    #[test]
    fn unmount_frees_the_slot_for_the_next_mount() {
        let mut app = make_test_app();
        for id in ["a", "b", "c"] {
            register_view(&mut app, id, true, &[]);
        }
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "a", Some(test_anchor()));
        mount(&mut app, terminal, "b", Some(test_anchor()));
        unmount(&mut app, terminal, Some("a"));
        assert_eq!(
            inline_children_of(&app, terminal).len(),
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
        for id in ["a", "b"] {
            register_view(&mut app, id, true, &[]);
        }
        let terminal = spawn_terminal(&mut app);
        mount(&mut app, terminal, "a", Some(test_anchor()));
        mount(&mut app, terminal, "b", Some(test_anchor()));
        let children = inline_children_of(&app, terminal);
        assert_eq!(children.len(), 2);

        unmount(&mut app, terminal, None);

        assert!(
            inline_children_of(&app, terminal).is_empty(),
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
        register_view(&mut app, "hud", false, &[]);
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "hud", Some(test_anchor()));

        let children = inline_children_of(&app, terminal);
        assert_eq!(children.len(), 1);
        assert!(
            app.world().get::<NonInteractive>(children[0]).is_some(),
            "a non-interactive view must carry NonInteractive"
        );
    }

    #[test]
    fn mount_without_anchor_is_dropped() {
        let mut app = make_test_app();
        register_view(&mut app, "dash", true, &[]);
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "dash", None);

        assert!(
            inline_children_of(&app, terminal).is_empty(),
            "a mount-inline without an anchor must be dropped"
        );
    }

    #[test]
    fn mount_of_unregistered_view_is_dropped() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);

        mount(&mut app, terminal, "ghost", Some(test_anchor()));

        assert!(
            inline_children_of(&app, terminal).is_empty(),
            "a mount-inline for an unregistered view must be dropped"
        );
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
}
