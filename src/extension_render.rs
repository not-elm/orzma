//! CEF integration for extension activities: the `ozmux-ext://` asset scheme and
//! the `window.ozmux` JS extension, the webview spawn-once system that attaches
//! a `bevy_cef` webview to each Extension Activity host, and the handler RPC
//! bridge (Task 9) that routes `window.ozmux` frames between the page and the
//! extension's handlers socket.

use crate::extension_manager::ExtensionRegistry;
use crate::system_set::OzmuxSystems;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::{ExtensionActivityMarker, HostActivityEntity};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::HandlersBridge;
use ozmux_extension_host::host::EndpointRegistry;
use ozmux_extension_host::scheme::custom_scheme;
use ozmux_multiplexer::{
    AttachedSession, ExtensionActivityAid, MultiplexerCommands, OwningExtension, SessionMarker,
};

/// Builds the `ozmux-ext://<name>/index.html` webview URL for an extension. The
/// `<name>` host segment dispatches through the shared `EndpointRegistry` in the
/// scheme handler; frames for unregistered names 404.
fn webview_url(extension_name: &str) -> String {
    format!("ozmux-ext://{extension_name}/index.html")
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

/// Owns the per-activity handler-socket connections and the shared outbound
/// channel that `drain_handler_responses` pumps back to the page.
#[derive(Resource, Default)]
struct ExtensionHandlersBridge(HandlersBridge);

/// `aid → webview entity` map, populated by the inbound observer the first time
/// an activity emits a frame, and read by the outbound drain to address a
/// `HostEmitEvent` at the originating webview.
// TODO: multi-activity — prune WebviewAidMap + call HandlersBridge::disconnect(aid) on activity close (RemovedComponents<ActivityMarker>); for the single memo activity this holds one entry.
#[derive(Resource, Default)]
struct WebviewAidMap(HashMap<String, Entity>);

/// Marks an extension-activity host whose webview could not be mounted because
/// its activity has no `OwningExtension`. Excluding marked hosts from the
/// `finish_extension_setup` query makes its diagnostic fire once, not every frame.
#[derive(Component)]
struct WebviewMountUnresolved;

/// JS defining `window.ozmux` over `cef.emit` / `cef.listen`, injected per
/// webview as a `PreloadScripts` entry (see `finish_extension_setup`). Mirrors
/// `sdk/typescript/src/cef/ozmux-bridge.ts`.
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
/// Extension Activity host.
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
            .init_resource::<WebviewAidMap>()
            .add_observer(on_ozmux_frame)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error)
            .add_systems(
                Update,
                (
                    finish_extension_setup.in_set(OzmuxSystems::SetupActivity),
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
fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    mux: MultiplexerCommands,
    attached_session: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>,
    registry: Res<ActivityEntityRegistry>,
    webviews: Query<(), With<WebviewSource>>,
) {
    let active = active_webview(&mux, &attached_session, &registry, &webviews);
    if focused.0 != active {
        focused.0 = active;
    }
}

/// The active pane's webview host entity, or `None` when the active activity is
/// not a webview (e.g. a terminal pane).
fn active_webview(
    mux: &MultiplexerCommands,
    attached_session: &Query<Entity, (With<SessionMarker>, With<AttachedSession>)>,
    registry: &ActivityEntityRegistry,
    webviews: &Query<(), With<WebviewSource>>,
) -> Option<Entity> {
    let session = attached_session.iter().next()?;
    let pane = mux.sessions_active_pane(session)?;
    let activity = mux.panes_active_activity(pane)?;
    let host = registry.get(activity)?;
    webviews.contains(host).then_some(host)
}

/// Attaches a `bevy_cef` webview to each Extension Activity host once its pane
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
    activity_hosts: Query<&HostActivityEntity>,
    owners: Query<&OwningExtension>,
    hosts: Query<
        (Entity, &ComputedNode),
        (
            With<ExtensionActivityMarker>,
            Without<WebviewSource>,
            Without<WebviewMountUnresolved>,
        ),
    >,
) {
    for (host, computed) in hosts.iter() {
        let Some(logical) = pane_logical_size(computed.size(), computed.inverse_scale_factor())
        else {
            continue;
        };
        let Some((session, pane, activity)) = host_multiplexer_chain(host, &activity_hosts, &mux)
        else {
            continue;
        };
        let Ok(owner) = owners.get(activity) else {
            tracing::warn!(
                ?host,
                ?activity,
                "extension activity has no OwningExtension; webview cannot be mounted (terminal-kind split over control socket?)"
            );
            commands.entity(host).insert(WebviewMountUnresolved);
            continue;
        };
        let name = owner.0.as_str();
        let url = webview_url(name);
        tracing::debug!(?host, ?logical, %url, "spawning extension webview");
        // NOTE: `window.ozmux` MUST be a PreloadScript, not a global CefExtension.
        // ozmux.js calls cef.listen() at top level; a global extension runs that
        // during V8 context creation, where there is no entered V8 context, so the
        // native cef.listen handler's v8_context_get_current_context() crashes the
        // render process. PreloadScripts are eval'd at on_context_created inside an
        // entered context (and their exceptions are caught, not fatal), so
        // cef.listen registers correctly there.
        let ctx_js = context_preload_js(session, pane, activity, name);
        commands.entity(host).insert((
            WebviewSource::new(url),
            WebviewSize(logical),
            PreloadScripts::from([ctx_js, OZMUX_EXTENSION_JS.to_string()]),
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
    }
}

/// Resolves the `(session, pane, activity)` multiplexer entities backing an
/// extension webview host: host → activity via `HostActivityEntity`, activity →
/// pane via `pane_of_activity`, pane → session via `session_of_pane`. Returns
/// `None` until every link exists (e.g. before the activity is laid out into a
/// pane).
fn host_multiplexer_chain(
    host: Entity,
    activity_hosts: &Query<&HostActivityEntity>,
    mux: &MultiplexerCommands,
) -> Option<(Entity, Entity, Entity)> {
    let activity = activity_hosts.get(host).ok()?.0;
    let pane = mux.pane_of_activity(activity)?;
    let session = mux.session_of_pane(pane)?;
    Some((session, pane, activity))
}

/// Builds the per-webview context PreloadScript assigning `window.__ozmuxContext`.
///
/// NOTE: PreloadScripts are joined with `;` and eval'd as one unit, so this MUST
/// be a complete statement; a syntax error here would break the bridge eval too.
fn context_preload_js(
    session: Entity,
    pane: Entity,
    activity: Entity,
    extension_name: &str,
) -> String {
    let session_id = session.to_bits().to_string();
    format!(
        "window.__ozmuxContext={{sessionId:{s:?},windowId:{s:?},paneId:{p:?},activityId:{a:?},role:\"extension\",extensionName:{n:?}}};",
        s = session_id,
        p = pane.to_bits().to_string(),
        a = activity.to_bits().to_string(),
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

/// Resolves the SDK activity id (`aid`) for the webview entity that emitted a
/// frame: the webview entity is the Extension Activity host, so its
/// `HostActivityEntity` points at the multiplexer Activity entity carrying the
/// `ExtensionActivityAid`. Returns `None` when either link is missing (e.g. the
/// activity has not yet been stamped by the control bridge).
fn aid_for_webview(
    webview: Entity,
    hosts: &Query<&HostActivityEntity>,
    aids: &Query<&ExtensionActivityAid>,
) -> Option<String> {
    let activity = hosts.get(webview).ok()?.0;
    Some(aids.get(activity).ok()?.0.clone())
}

/// Inbound: a `window.ozmux` `cef.emit(frame)` arrives as `Receive<OzmuxFrame>`
/// targeting the emitting webview. Resolves the webview's `aid` and its owning
/// extension's handlers socket (via the activity's `OwningExtension` and the
/// `ExtensionRegistry`), connects idempotently, records the `aid → webview`
/// mapping for the outbound path, and forwards the frame.
///
/// Frames are dropped when the webview cannot be resolved to an `aid`/owner
/// (the activity has not been stamped yet) or the owning extension is not in
/// the registry (failed to launch) — there is no handler set to address.
fn on_ozmux_frame(
    frame: On<Receive<OzmuxFrame>>,
    bridge: Res<ExtensionHandlersBridge>,
    registry: Res<ExtensionRegistry>,
    mut aid_map: ResMut<WebviewAidMap>,
    hosts: Query<&HostActivityEntity>,
    owners: Query<&OwningExtension>,
    aids: Query<&ExtensionActivityAid>,
) {
    let webview = frame.webview;
    let Some(aid) = aid_for_webview(webview, &hosts, &aids) else {
        return;
    };
    let Ok(owner) = hosts.get(webview).and_then(|h| owners.get(h.0)) else {
        return;
    };
    let Some(ext) = registry.extensions.get(&owner.0) else {
        return;
    };
    let sock = ext.handlers_sock_path().to_path_buf();
    if let Err(e) = bridge.0.connect(aid.clone(), sock) {
        tracing::warn!(%aid, error = %e, "extension handlers connect failed");
        return;
    }
    aid_map.0.insert(aid.clone(), webview);
    if let Ok(frame_json) = serde_json::to_string(&frame.payload.0) {
        bridge.0.send(&aid, frame_json);
    }
}

/// Outbound: drains handler responses `(aid, frame)` and re-emits each to the
/// originating webview as a `HostEmitEvent` on the `"ozmux"` channel, which the
/// page's `cef.listen('ozmux', …)` receives (as a JSON string it `JSON.parse`s).
/// Non-blocking; responses for an unmapped `aid` (no inbound seen yet) are
/// dropped.
fn drain_handler_responses(
    bridge: Res<ExtensionHandlersBridge>,
    aid_map: Res<WebviewAidMap>,
    mut commands: Commands,
) {
    while let Ok((aid, frame)) = bridge.0.outbound().try_recv() {
        let Some(&webview) = aid_map.0.get(&aid) else {
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

    /// Spawns a session/pane/extension-activity chain and an extension host
    /// entity carrying that activity via `HostActivityEntity`, returning the
    /// `(host, session, pane, activity)` handles. `finish_extension_setup`
    /// needs the chain to resolve the per-webview context.
    fn spawn_extension_host(app: &mut App, extra: impl Bundle) -> (Entity, Entity, Entity, Entity) {
        let (session, pane, activity) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_session(Some("t".into()));
                (o.session, o.pane, o.activity)
            })
            .unwrap();
        app.world_mut()
            .entity_mut(activity)
            .insert(OwningExtension("memo".into()));
        app.world_mut().flush();
        let host = app
            .world_mut()
            .spawn((ExtensionActivityMarker, HostActivityEntity(activity), extra))
            .id();
        (host, session, pane, activity)
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
        app.init_resource::<ActivityEntityRegistry>();
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let (session, terminal_pane) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_session(Some("t".into()));
                (o.session, o.pane)
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
        let (terminal_activity, ext_activity) = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                (
                    mux.panes_active_activity(terminal_pane)
                        .expect("terminal activity"),
                    mux.panes_active_activity(ext_pane).expect("ext activity"),
                )
            })
            .unwrap();
        app.world_mut().entity_mut(session).insert(AttachedSession);

        // Terminal host: no WebviewSource. Extension host: carries WebviewSource.
        let terminal_host = app.world_mut().spawn_empty().id();
        let ext_host = app
            .world_mut()
            .spawn(WebviewSource::new(webview_url("memo")))
            .id();
        {
            let mut reg = app.world_mut().resource_mut::<ActivityEntityRegistry>();
            reg.insert_for_test(terminal_activity, terminal_host);
            reg.insert_for_test(ext_activity, ext_host);
        }

        let set_active = move |app: &mut App, pane: Entity| {
            app.world_mut()
                .run_system_once(move |mut mux: MultiplexerCommands| {
                    mux.set_active_pane(session, pane).expect("set_active_pane");
                })
                .unwrap();
            app.world_mut().flush();
            app.update();
        };

        set_active(&mut app, ext_pane);
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(ext_host),
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
            "entity without ExtensionActivityMarker must not receive a WebviewSource"
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
            WebviewSource::Url(url) => assert_eq!(url, &webview_url("memo")),
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
    fn warns_once_and_marks_host_when_activity_lacks_owning_extension() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);

        let activity = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("t".into())).activity
            })
            .unwrap();
        app.world_mut().flush();
        let host = app
            .world_mut()
            .spawn((
                ExtensionActivityMarker,
                HostActivityEntity(activity),
                laid_out_node(Vec2::new(800.0, 600.0)),
            ))
            .id();

        app.update();
        assert!(
            app.world().get::<WebviewSource>(host).is_none(),
            "a host whose activity lacks OwningExtension must not get a webview"
        );
        assert!(
            app.world().get::<WebviewMountUnresolved>(host).is_some(),
            "the host must be marked so the diagnostic fires once, not every frame"
        );

        // A second tick must not re-process the marked host (still no webview).
        app.update();
        assert!(
            app.world().get::<WebviewSource>(host).is_none(),
            "the marked host must stay excluded from the query"
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
    fn context_preload_js_assigns_window_context_with_session_bits_as_window_id() {
        let world = &mut App::new();
        world
            .add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        let (session, pane, activity) = world
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_session(Some("t".into()));
                (o.session, o.pane, o.activity)
            })
            .unwrap();
        world.world_mut().flush();

        let js = context_preload_js(session, pane, activity, "memo");
        let s = session.to_bits().to_string();
        assert!(js.starts_with("window.__ozmuxContext="));
        assert!(js.ends_with("};"));
        assert!(js.contains(&format!("sessionId:\"{s}\"")));
        assert!(
            js.contains(&format!("windowId:\"{s}\"")),
            "windowId must equal sessionId per the design"
        );
        assert!(js.contains(&format!("paneId:\"{}\"", pane.to_bits())));
        assert!(js.contains(&format!("activityId:\"{}\"", activity.to_bits())));
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
    fn aid_for_webview_resolves_through_host_activity_entity() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = make_test_app();
        let world = app.world_mut();
        let activity = world.spawn(ExtensionActivityAid("aid-42".into())).id();
        let webview = world.spawn(HostActivityEntity(activity)).id();
        let stray = world.spawn_empty().id();

        let resolved = world
            .run_system_once(
                move |hosts: Query<&HostActivityEntity>, aids: Query<&ExtensionActivityAid>| {
                    (
                        aid_for_webview(webview, &hosts, &aids),
                        aid_for_webview(stray, &hosts, &aids),
                    )
                },
            )
            .unwrap();

        assert_eq!(resolved.0.as_deref(), Some("aid-42"));
        assert_eq!(
            resolved.1, None,
            "a webview with no HostActivityEntity must resolve to no aid"
        );
    }

    #[test]
    fn aid_for_webview_is_none_when_activity_lacks_aid() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = make_test_app();
        let world = app.world_mut();
        let activity = world.spawn_empty().id();
        let webview = world.spawn(HostActivityEntity(activity)).id();

        let resolved = world
            .run_system_once(
                move |hosts: Query<&HostActivityEntity>, aids: Query<&ExtensionActivityAid>| {
                    aid_for_webview(webview, &hosts, &aids)
                },
            )
            .unwrap();

        assert_eq!(
            resolved, None,
            "an unstamped activity (no ExtensionActivityAid) must resolve to no aid"
        );
    }
}
