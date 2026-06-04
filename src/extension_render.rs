//! CEF integration for extension surfaces: the `ozmux-ext://` asset scheme and
//! the `window.ozmux` JS extension, the webview spawn-once system that attaches
//! a `bevy_cef` webview to each Extension Surface host, and the handler RPC
//! bridge (Task 9) that routes `window.ozmux` frames between the page and the
//! extension's handlers socket.

use crate::extension_manager::ExtensionRegistry;
use crate::system_set::OzmuxSystems;
use crate::ui::{AddressBarFocus, BrowserPageWebview, ExtensionSurfaceMarker};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::HandlersBridge;
use ozmux_extension_host::host::EndpointRegistry;
use ozmux_extension_host::scheme::custom_scheme;
use ozmux_multiplexer::{
    AttachedWorkspace, ExtensionSurfaceId, MultiplexerCommands, OwningExtension, SurfaceKind,
    WorkspaceMarker,
};

/// Builds the `ozmux-ext://<name>/<entry>` webview URL for an extension surface,
/// where `entry` is the client's HTML path relative to the extension dir.
fn webview_url(extension_name: &str, entry: &str) -> String {
    format!("ozmux-ext://{extension_name}/{entry}")
}

/// One handler/channel frame emitted by the page's `window.ozmux` (the JSON the
/// SDK handlers-server speaks). Carried verbatim to the handlers bridge.
///
/// `#[serde(transparent)]` makes it deserialize from the bare emitted object
/// (`{kind, id, name, payload}`), not from a `{"0": …}` wrapper — `bevy_cef`'s
/// `cef.emit(frame)` serializes only its first argument into one global
/// `Receive<OzmuxFrame>`.
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
struct OzmuxFrame(serde_json::Value);

/// Owns the per-surface handler-socket connections and the shared outbound
/// channel that `drain_handler_responses` pumps back to the page.
#[derive(Resource, Default)]
struct ExtensionHandlersBridge(HandlersBridge);

/// `surface_id → webview entity` map, populated by the inbound observer the first time
/// a surface emits a frame, and read by the outbound drain to address a
/// `HostEmitEvent` at the originating webview.
// TODO: multi-surface — prune WebviewSurfaceIdMap + call HandlersBridge::disconnect(surface_id) on surface close (RemovedComponents<SurfaceMarker>); for the single memo surface this holds one entry.
#[derive(Resource, Default)]
struct WebviewSurfaceIdMap(HashMap<String, Entity>);

/// Marks an extension-surface host whose webview could not be mounted because
/// its surface has no `OwningExtension`. Excluding marked hosts from the
/// `finish_extension_setup` query makes its diagnostic fire once, not every frame.
#[derive(Component)]
struct WebviewMountUnresolved;

/// JS defining `window.ozmux` over `cef.emit` / `cef.listen`, injected per
/// webview as a `PreloadScripts` entry (see `finish_extension_setup`). Mirrors
/// `sdk/typescript/src/surface/ozmux-bridge.ts`.
pub const OZMUX_EXTENSION_JS: &str = include_str!("extension_render/ozmux.js");

/// Builds the `CefPlugin` with the `ozmux-ext://` scheme bound to the shared
/// `EndpointRegistry` the extension manager populates per extension on launch.
/// The handler reads the live registry on each request, so endpoints registered
/// after `CefPlugin::build()` resolve; frames for unregistered names 404. The
/// `window.ozmux` bridge is intentionally NOT registered as a global extension
/// here; it is injected per-webview via `PreloadScripts` in
/// `finish_extension_setup` (see the NOTE there).
pub fn cef_plugin(endpoints: EndpointRegistry) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_scheme(endpoints)],
        command_line_config: cef_command_line_config(),
        ..Default::default()
    }
}

/// CEF command-line switches for the embedded webview. The `debug` feature adds
/// `remote-debugging-port` — a local Chromium DevTools (CDP) endpoint on
/// `127.0.0.1:9222` for inspecting the extension webview — and is off by default
/// so that endpoint is never exposed in normal builds. `CommandLineConfig::default()`
/// already carries the macOS `use-mock-keychain` switch in either case.
fn cef_command_line_config() -> CommandLineConfig {
    let config = CommandLineConfig::default();
    #[cfg(feature = "debug")]
    let config = config.with_switch_value("remote-debugging-port", "9222");
    config
}

/// Wires the spawn-once system that attaches a `bevy_cef` webview to each
/// Extension Surface host.
///
/// `finish_extension_setup` seeds each webview's INITIAL `WebviewSize` exactly
/// once, at creation, from its laid-out pane (see that fn for why). ONGOING
/// resize is intentionally NOT handled here: `bevy_cef`'s `UiWebviewPlugin`
/// (pulled in by `CefPlugin`) already runs `update_webview_ui_size` in
/// `PostUpdate` after `UiSystems::Layout`, keeping each UI webview's
/// `WebviewSize` in step with its `ComputedNode` on every layout pass. The
/// one-time seed and that per-frame sync do not conflict — the seed equals the
/// first synced value, so `update_webview_ui_size`'s `set_if_neq` is a no-op.
/// The terminal path needs its own resize only because it derives grid
/// `cols`/`rows` from font metrics, which `bevy_cef` knows nothing about.
pub struct OzmuxExtensionRenderPlugin;

impl Plugin for OzmuxExtensionRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JsEmitEventPlugin::<OzmuxFrame>::default())
            .init_resource::<ExtensionHandlersBridge>()
            .init_resource::<WebviewSurfaceIdMap>()
            .add_observer(on_ozmux_frame)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error)
            .add_systems(
                Update,
                (
                    finish_extension_setup.in_set(OzmuxSystems::SetupSurface),
                    drain_handler_responses,
                    sync_focused_webview.after(OzmuxSystems::Input),
                ),
            );
    }
}

/// Keeps `bevy_cef`'s `FocusedWebview` in step with ozmux's active pane.
///
/// bevy_cef only updates `FocusedWebview` when a *webview* node is clicked
/// (`set_focus_on_press`), so moving focus to a terminal pane (a non-webview)
/// leaves the extension webview focused: its DOM text area keeps the caret and
/// `send_key_event` keeps routing keystrokes to it. Driving `FocusedWebview`
/// from the active pane fixes both — keyboard follows the focused pane, and CEF
/// blurs the webview on focus-leave (`bevy_cef`'s `apply_webview_focus` releases
/// CEF focus when `FocusedWebview` becomes `None`).
///
/// For browser surfaces the webview lives on a child entity pointed to by
/// `BrowserPageWebview`; `active_webview` resolves through that indirection.
/// When `AddressBarFocus` names the active surface, CEF focus is released so
/// the address bar can own the keyboard.
fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    webviews: Query<(), With<WebviewSource>>,
    browser_hosts: Query<&BrowserPageWebview>,
    address_focus: Option<Res<AddressBarFocus>>,
) {
    let bar_focused_surface = address_focus.as_ref().and_then(|f| f.0);
    let active = active_webview(
        &mux,
        &attached_workspace,
        &webviews,
        &browser_hosts,
        bar_focused_surface,
    );
    if focused.0 != active {
        focused.0 = active;
    }
}

/// The active pane's focused webview entity, or `None` when the active surface
/// is not a webview (e.g. a terminal pane) or when the address bar owns input.
///
/// For extension surfaces the webview is on the Surface entity itself
/// (`WebviewSource`). For browser surfaces the webview is on the
/// `BrowserPageWebview` child; when `bar_focused_surface` matches the Surface,
/// returns `None` to release CEF focus.
fn active_webview(
    mux: &MultiplexerCommands,
    attached_workspace: &Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    webviews: &Query<(), With<WebviewSource>>,
    browser_hosts: &Query<&BrowserPageWebview>,
    bar_focused_surface: Option<Entity>,
) -> Option<Entity> {
    let workspace = attached_workspace.iter().next()?;
    let pane = mux.workspaces_active_pane(workspace)?;
    let surface = mux.panes_active_surface(pane)?;
    if webviews.contains(surface) {
        return Some(surface);
    }
    if let Ok(page) = browser_hosts.get(surface) {
        if bar_focused_surface == Some(surface) {
            return None;
        }
        return Some(page.0);
    }
    None
}

/// Attaches a `bevy_cef` webview to each Extension Surface host once its pane
/// has a real laid-out size: a `WebviewSource` pointed at the memo extension, a
/// `WebviewSize` seeded from the host's `ComputedNode`, and a
/// `MaterialNode<WebviewUiMaterial>`. Runs every Update tick but skips a host
/// until its `ComputedNode` reports a real (≥ 1 logical px) size, and only
/// targets hosts that lack `WebviewSource`, so the per-entity insertion happens
/// exactly once.
///
/// Seeding `WebviewSize` at insert time is load-bearing. `bevy_cef`'s
/// `create_webview` reads `WebviewSize` when it builds the CEF browser, and the
/// component defaults to 800×800. If the webview were inserted before layout,
/// the browser would be created at 800×800 and then resized to the real pane
/// size a frame later (when `update_webview_ui_size` syncs `WebviewSize` from
/// `ComputedNode`). That mid-load `was_resized()` races CEF's offscreen
/// renderer-widget init and wedges it (`blink.mojom.Widget` message rejections →
/// no `LoadFinished`, no paint → a permanently white pane). By waiting for
/// layout and creating the browser at the final size, the first
/// `update_webview_ui_size` pass is a `set_if_neq` no-op, so no resize fires
/// during the load.
fn finish_extension_setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<WebviewUiMaterial>>,
    mux: MultiplexerCommands,
    owners: Query<&OwningExtension>,
    kinds: Query<&SurfaceKind>,
    surfaces: Query<
        (Entity, &ComputedNode),
        (
            With<ExtensionSurfaceMarker>,
            Without<WebviewSource>,
            Without<WebviewMountUnresolved>,
        ),
    >,
) {
    for (surface, computed) in surfaces.iter() {
        let Some(logical) = pane_logical_size(computed.size(), computed.inverse_scale_factor())
        else {
            continue;
        };
        let Some((workspace, pane)) = surface_multiplexer_chain(surface, &mux) else {
            continue;
        };
        let Ok(owner) = owners.get(surface) else {
            tracing::warn!(
                ?surface,
                "extension surface has no OwningExtension; webview cannot be mounted (terminal-kind split over control socket?)"
            );
            commands.entity(surface).insert(WebviewMountUnresolved);
            continue;
        };
        let Ok(SurfaceKind::Extension { entry }) = kinds.get(surface) else {
            continue;
        };
        let entry = entry.to_string_lossy();
        let name = owner.0.as_str();
        let url = webview_url(name, &entry);
        tracing::debug!(?surface, ?logical, %url, "spawning extension webview");
        // NOTE: `window.ozmux` MUST be a PreloadScript, not a global CefExtension.
        // ozmux.js calls cef.listen() at top level; a global extension runs that
        // during V8 context creation, where there is no entered V8 context, so the
        // native cef.listen handler's v8_context_get_current_context() crashes the
        // render process. PreloadScripts are eval'd at on_context_created inside an
        // entered context (and their exceptions are caught, not fatal), so
        // cef.listen registers correctly there.
        let ctx_js = context_preload_js(workspace, pane, surface, name);
        commands.entity(surface).insert((
            WebviewSource::new(url),
            WebviewSize(logical),
            PreloadScripts::from([ctx_js, OZMUX_EXTENSION_JS.to_string()]),
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
    }
}

/// Resolves the `(workspace, pane)` multiplexer entities owning an extension
/// Surface: surface → pane via `pane_of_surface`, pane → workspace via
/// `workspace_of_pane`. Returns `None` until every link exists (e.g. before
/// the surface is laid out into a pane).
fn surface_multiplexer_chain(
    surface: Entity,
    mux: &MultiplexerCommands,
) -> Option<(Entity, Entity)> {
    let pane = mux.pane_of_surface(surface)?;
    let workspace = mux.workspace_of_pane(pane)?;
    Some((workspace, pane))
}

/// Builds the per-webview context PreloadScript assigning `window.__ozmuxContext`.
///
/// NOTE: PreloadScripts are joined with `;` and eval'd as one unit, so this MUST
/// be a complete statement; a syntax error here would break the bridge eval too.
fn context_preload_js(
    workspace: Entity,
    pane: Entity,
    surface: Entity,
    extension_name: &str,
) -> String {
    let workspace_id = workspace.to_bits().to_string();
    // NOTE: the JS keys "sessionId"/"windowId" keep their legacy names on purpose — a
    // browser-side wire contract the SDK surface client reads; renaming them breaks extensions.
    format!(
        "window.__ozmuxContext={{sessionId:{s:?},windowId:{s:?},paneId:{p:?},surfaceId:{a:?},role:\"extension\",extensionName:{n:?}}};",
        s = workspace_id,
        p = pane.to_bits().to_string(),
        a = surface.to_bits().to_string(),
        n = extension_name,
    )
}

/// Converts a host pane's `ComputedNode` physical-pixel size to the logical
/// (DIP) size `WebviewSize` expects, or `None` when the pane has no real area
/// yet (pre-layout / sub-pixel). Mirrors `bevy_cef`'s `webview_size_from_computed`,
/// duplicated here because that fn is `pub(crate)`.
fn pane_logical_size(physical: Vec2, inverse_scale_factor: f32) -> Option<Vec2> {
    let logical = physical * inverse_scale_factor;
    if logical.x < 1.0 || logical.y < 1.0 {
        None
    } else {
        Some(logical)
    }
}

/// Resolves the SDK surface id (`surface_id`) for the webview entity that
/// emitted a frame: the webview entity *is* the Extension Surface, so it
/// carries the `ExtensionSurfaceId` directly. Returns `None` when the surface
/// has not yet been stamped by the control bridge.
fn surface_id_for_webview(
    webview: Entity,
    surface_ids: &Query<&ExtensionSurfaceId>,
) -> Option<String> {
    Some(surface_ids.get(webview).ok()?.0.clone())
}

/// Inbound: a `window.ozmux` `cef.emit(frame)` arrives as `Receive<OzmuxFrame>`
/// targeting the emitting webview. Resolves the webview's `surface_id` and its owning
/// extension's handlers socket (via the surface's `OwningExtension` and the
/// `ExtensionRegistry`), connects idempotently, records the `surface_id → webview`
/// mapping for the outbound path, and forwards the frame.
///
/// Frames are dropped when the webview cannot be resolved to a `surface_id`/owner
/// (the surface has not been stamped yet) or the owning extension is not in
/// the registry (failed to launch) — there is no handler set to address.
fn on_ozmux_frame(
    frame: On<Receive<OzmuxFrame>>,
    bridge: Res<ExtensionHandlersBridge>,
    registry: Res<ExtensionRegistry>,
    mut surface_id_map: ResMut<WebviewSurfaceIdMap>,
    owners: Query<&OwningExtension>,
    surface_ids: Query<&ExtensionSurfaceId>,
) {
    let webview = frame.webview;
    let Some(surface_id) = surface_id_for_webview(webview, &surface_ids) else {
        return;
    };
    let Ok(owner) = owners.get(webview) else {
        return;
    };
    let Some(ext) = registry.extensions.get(&owner.0) else {
        return;
    };
    let sock = ext.handlers_sock_path().to_path_buf();
    if let Err(e) = bridge.0.connect(surface_id.clone(), sock) {
        tracing::warn!(%surface_id, error = %e, "extension handlers connect failed");
        return;
    }
    surface_id_map.0.insert(surface_id.clone(), webview);
    if let Ok(frame_json) = serde_json::to_string(&frame.payload.0) {
        bridge.0.send(&surface_id, frame_json);
    }
}

/// Outbound: drains handler responses `(surface_id, frame)` and re-emits each to the
/// originating webview as a `HostEmitEvent` on the `"ozmux"` channel, which the
/// page's `cef.listen('ozmux', …)` receives (as a JSON string it `JSON.parse`s).
/// Non-blocking; responses for an unmapped `surface_id` (no inbound seen yet) are
/// dropped.
fn drain_handler_responses(
    bridge: Res<ExtensionHandlersBridge>,
    surface_id_map: Res<WebviewSurfaceIdMap>,
    mut commands: Commands,
) {
    while let Ok((surface_id, frame)) = bridge.0.outbound().try_recv() {
        let Some(&webview) = surface_id_map.0.get(&surface_id) else {
            continue;
        };
        let value: serde_json::Value =
            serde_json::from_str(&frame).unwrap_or(serde_json::Value::Null);
        commands.trigger(HostEmitEvent::new(webview, "ozmux", &value));
    }
}

/// Logs the start of a webview page load. Debug-level diagnostics: these
/// observers fire for every `bevy_cef` webview, not only extension hosts.
fn log_webview_load_started(load: On<LoadStarted>) {
    tracing::debug!(webview = ?load.webview, "webview load started");
}

/// Logs a finished page load + its HTTP status. A `LoadFinished` with no
/// visible content points at a render/size issue rather than a load failure.
fn log_webview_load_finished(load: On<LoadFinished>) {
    tracing::debug!(
        webview = ?load.webview,
        status = load.http_status_code,
        "webview load finished"
    );
}

/// Logs a page load failure (CEF `OnLoadError`) — the signal that the scheme
/// fetch / navigation failed (e.g. a mis-classified MIME or 5xx). Kept at
/// `warn` because, unlike start/finish, it always indicates a real fault.
fn log_webview_load_error(load: On<LoadError>) {
    tracing::warn!(
        webview = ?load.webview,
        code = load.error_code,
        url = %load.url,
        "webview load error"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::image::ImagePlugin;
    use ozmux_multiplexer::MultiplexerPlugin;

    fn make_test_app() -> App {
        // NOTE: `bevy_cef`'s `UiWebviewPlugin` registers `WebviewUiMaterial`
        // through `UiMaterialPlugin`, which pulls in the full render stack. For
        // these headless tests we only need `Assets<WebviewUiMaterial>` to exist
        // so the system's `ResMut<Assets<...>>` parameter resolves. The material
        // is a plain `Asset` (no render-app init required), so `init_asset`
        // suffices — mirrors `ui::terminal`'s `TerminalUiMaterial` test setup.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .add_plugins(MultiplexerPlugin)
            .init_asset::<WebviewUiMaterial>();
        app
    }

    /// Spawns a workspace/pane/extension-surface chain and decorates the
    /// Surface entity (which *is* its own host) with `ExtensionSurfaceMarker`
    /// + `extra`, returning the `(surface, workspace, pane)` handles.
    /// `finish_extension_setup` needs the chain to resolve the per-webview
    /// context. The surface is stamped with
    /// `SurfaceKind::Extension { entry: "ui/app.html" }` and
    /// `OwningExtension("memo")`.
    fn spawn_extension_host(app: &mut App, extra: impl Bundle) -> (Entity, Entity, Entity) {
        use std::path::PathBuf;
        let (workspace, pane, surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().entity_mut(surface).insert((
            OwningExtension("memo".into()),
            SurfaceKind::Extension {
                entry: PathBuf::from("ui/app.html"),
            },
            ExtensionSurfaceMarker,
            extra,
        ));
        app.world_mut().flush();
        (surface, workspace, pane)
    }

    #[test]
    fn focused_webview_follows_active_pane() {
        // Regression: moving focus to a terminal pane must clear FocusedWebview,
        // so bevy_cef blurs the extension webview (releasing its DOM text area
        // and stopping keyboard from routing to it). When the extension pane is
        // active, its webview must be focused.
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin, Side, SplitOrientation};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let (workspace, terminal_pane) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane)
            })
            .unwrap();
        app.world_mut().flush();
        let ext_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(terminal_pane, Side::After, SplitOrientation::Horizontal)
                    .expect("split_pane")
            })
            .unwrap();
        app.world_mut().flush();
        let (terminal_surface, ext_surface) = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                (
                    mux.panes_active_surface(terminal_pane)
                        .expect("terminal surface"),
                    mux.panes_active_surface(ext_pane).expect("ext surface"),
                )
            })
            .unwrap();
        app.world_mut()
            .entity_mut(workspace)
            .insert(AttachedWorkspace);

        // The Surface entity IS its own host: the terminal surface carries no
        // WebviewSource; the extension surface carries one.
        let _ = terminal_surface;
        app.world_mut()
            .entity_mut(ext_surface)
            .insert(WebviewSource::new(webview_url("memo", "ui/app.html")));

        let set_active = move |app: &mut App, pane: Entity| {
            app.world_mut()
                .run_system_once(move |mut mux: MultiplexerCommands| {
                    mux.set_active_pane(workspace, pane)
                        .expect("set_active_pane");
                })
                .unwrap();
            app.world_mut().flush();
            app.update();
        };

        set_active(&mut app, ext_pane);
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(ext_surface),
            "active extension pane must focus its webview"
        );

        set_active(&mut app, terminal_pane);
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "moving focus to the terminal pane must clear the focused webview",
        );
    }

    fn laid_out_node(physical: Vec2) -> ComputedNode {
        ComputedNode {
            size: physical,
            inverse_scale_factor: 1.0,
            ..ComputedNode::DEFAULT
        }
    }

    #[test]
    fn skips_entities_without_extension_marker() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app
            .world_mut()
            .spawn(laid_out_node(Vec2::new(800.0, 600.0)))
            .id();
        app.update();
        assert!(
            app.world().get::<WebviewSource>(host).is_none(),
            "entity without ExtensionSurfaceMarker must not receive a WebviewSource"
        );
    }

    #[test]
    fn attaches_webview_pointed_at_memo_to_extension_host() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let (host, ..) = spawn_extension_host(&mut app, laid_out_node(Vec2::new(800.0, 600.0)));
        app.update();

        let source = app
            .world()
            .get::<WebviewSource>(host)
            .expect("extension host must receive a WebviewSource");
        match source {
            WebviewSource::Url(url) => {
                assert_eq!(url, &webview_url("memo", "ui/app.html"));
                assert_eq!(url, "ozmux-ext://memo/ui/app.html");
            }
            other => panic!("expected a Url source, got {other:?}"),
        }
        assert!(
            app.world()
                .get::<MaterialNode<WebviewUiMaterial>>(host)
                .is_some(),
            "extension host must receive a WebviewUiMaterial MaterialNode"
        );
        assert_eq!(
            app.world().get::<WebviewSize>(host).map(|s| s.0),
            Some(Vec2::new(800.0, 600.0)),
            "the webview must be seeded with the pane's laid-out logical size, not the 800x800 default"
        );
        let preload = app
            .world()
            .get::<PreloadScripts>(host)
            .expect("the webview must carry the window.ozmux bridge as a PreloadScript");
        assert!(
            preload.0.iter().any(|s| s == OZMUX_EXTENSION_JS),
            "window.ozmux must be injected as a PreloadScript (a global CefExtension calling cef.listen at load crashes the renderer)"
        );
        assert!(
            preload
                .0
                .first()
                .is_some_and(|s| s.starts_with("window.__ozmuxContext=")),
            "the context PreloadScript must be injected before the bridge, so window.__ozmuxContext is set when the getter reads it"
        );
        assert!(
            preload.0[0].contains("role:\"extension\"")
                && preload.0[0].contains("extensionName:\"memo\""),
            "the context PreloadScript must carry the extension role and name"
        );
    }

    #[test]
    fn warns_once_and_marks_host_when_surface_lacks_owning_extension() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);

        let surface = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("t".into())).surface
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut().entity_mut(surface).insert((
            ExtensionSurfaceMarker,
            laid_out_node(Vec2::new(800.0, 600.0)),
        ));

        app.update();
        assert!(
            app.world().get::<WebviewSource>(surface).is_none(),
            "a surface that lacks OwningExtension must not get a webview"
        );
        assert!(
            app.world().get::<WebviewMountUnresolved>(surface).is_some(),
            "the surface must be marked so the diagnostic fires once, not every frame"
        );

        // A second tick must not re-process the marked surface (still no webview).
        app.update();
        assert!(
            app.world().get::<WebviewSource>(surface).is_none(),
            "the marked surface must stay excluded from the query"
        );
    }

    #[test]
    fn defers_webview_until_pane_is_laid_out() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let (host, ..) = spawn_extension_host(&mut app, ComputedNode::DEFAULT);
        app.update();
        assert!(
            app.world().get::<WebviewSource>(host).is_none(),
            "a zero-area (pre-layout) host must not receive a webview yet"
        );

        app.world_mut()
            .entity_mut(host)
            .insert(laid_out_node(Vec2::new(640.0, 480.0)));
        app.update();
        assert!(
            app.world().get::<WebviewSource>(host).is_some(),
            "once the pane has a real size, the webview must be attached"
        );
    }

    #[test]
    fn webview_inserted_exactly_once() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let (host, ..) = spawn_extension_host(&mut app, laid_out_node(Vec2::new(800.0, 600.0)));
        app.update();
        let first = app
            .world()
            .get::<MaterialNode<WebviewUiMaterial>>(host)
            .map(|m| m.id());
        app.update();
        let second = app
            .world()
            .get::<MaterialNode<WebviewUiMaterial>>(host)
            .map(|m| m.id());
        assert_eq!(
            first, second,
            "the second tick must not re-insert (and so not replace) the webview material"
        );
    }

    #[test]
    fn pane_logical_size_rejects_zero_and_subpixel() {
        assert_eq!(pane_logical_size(Vec2::ZERO, 1.0), None);
        assert_eq!(pane_logical_size(Vec2::new(0.0, 600.0), 1.0), None);
        assert_eq!(pane_logical_size(Vec2::new(0.5, 0.5), 1.0), None);
    }

    #[test]
    fn context_preload_js_assigns_window_context_with_workspace_bits_as_window_id() {
        let world = &mut App::new();
        world
            .add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        let (workspace, pane, surface) = world
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        world.world_mut().flush();

        let js = context_preload_js(workspace, pane, surface, "memo");
        let s = workspace.to_bits().to_string();
        assert!(js.starts_with("window.__ozmuxContext="));
        assert!(js.ends_with("};"));
        assert!(js.contains(&format!("sessionId:\"{s}\"")));
        assert!(
            js.contains(&format!("windowId:\"{s}\"")),
            "windowId must equal sessionId per the design"
        );
        assert!(js.contains(&format!("paneId:\"{}\"", pane.to_bits())));
        assert!(js.contains(&format!("surfaceId:\"{}\"", surface.to_bits())));
        assert!(js.contains("role:\"extension\""));
        assert!(js.contains("extensionName:\"memo\""));
    }

    #[test]
    fn pane_logical_size_scales_physical_to_logical() {
        assert_eq!(
            pane_logical_size(Vec2::new(640.0, 480.0), 1.0),
            Some(Vec2::new(640.0, 480.0))
        );
        assert_eq!(
            pane_logical_size(Vec2::new(1600.0, 1200.0), 0.5),
            Some(Vec2::new(800.0, 600.0))
        );
    }

    #[test]
    fn ozmux_frame_deserializes_from_bare_emitted_object() {
        let raw = r#"{"kind":"call","id":"c0","name":"greet","payload":{"x":1}}"#;
        let frame: OzmuxFrame = serde_json::from_str(raw).expect("transparent newtype");
        assert_eq!(frame.0["kind"], "call");
        assert_eq!(frame.0["id"], "c0");
        assert_eq!(frame.0["name"], "greet");
        assert_eq!(frame.0["payload"]["x"], 1);
    }

    #[test]
    fn surface_id_for_webview_resolves_through_host_surface_entity() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = make_test_app();
        let world = app.world_mut();
        // The webview entity IS the Extension Surface; it carries the id directly.
        let surface = world.spawn(ExtensionSurfaceId("aid-42".into())).id();
        let stray = world.spawn_empty().id();

        let resolved = world
            .run_system_once(move |surface_ids: Query<&ExtensionSurfaceId>| {
                (
                    surface_id_for_webview(surface, &surface_ids),
                    surface_id_for_webview(stray, &surface_ids),
                )
            })
            .unwrap();

        assert_eq!(resolved.0.as_deref(), Some("aid-42"));
        assert_eq!(
            resolved.1, None,
            "a webview with no ExtensionSurfaceId must resolve to no surface_id"
        );
    }

    #[test]
    fn surface_id_for_webview_is_none_when_surface_lacks_surface_id() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = make_test_app();
        let world = app.world_mut();
        let surface = world.spawn_empty().id();

        let resolved = world
            .run_system_once(move |surface_ids: Query<&ExtensionSurfaceId>| {
                surface_id_for_webview(surface, &surface_ids)
            })
            .unwrap();

        assert_eq!(
            resolved, None,
            "an unstamped surface (no ExtensionSurfaceId) must resolve to no surface_id"
        );
    }

    #[test]
    fn focused_webview_resolves_browser_child_and_respects_address_focus() {
        use crate::ui::{AddressBarFocus, BrowserPageWebview};
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.init_resource::<AddressBarFocus>();
        app.add_systems(Update, sync_focused_webview);

        let (workspace, _pane, surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(workspace)
            .insert(AttachedWorkspace);

        // The Surface entity IS its own host; it owns the page-webview child.
        let child = app
            .world_mut()
            .spawn(WebviewSource::new("https://example.com"))
            .id();
        app.world_mut()
            .entity_mut(surface)
            .insert(BrowserPageWebview(child));

        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "active browser pane focuses its page-webview child"
        );

        app.world_mut().resource_mut::<AddressBarFocus>().0 = Some(surface);
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "address-bar focus releases CEF focus"
        );
    }
}
