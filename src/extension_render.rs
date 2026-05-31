//! CEF integration for extension activities: the `ozmux-ext://` asset scheme and
//! the `window.ozmux` JS extension, the webview spawn-once system that attaches
//! a `bevy_cef` webview to each Extension Activity host, and the handler RPC
//! bridge (Task 9) that routes `window.ozmux` frames between the page and the
//! extension's handlers socket.

use crate::system_set::OzmuxSystems;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::{ExtensionActivityMarker, HostActivityEntity};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::host::ExtensionEndpoints;
use ozmux_extension_host::scheme::custom_scheme;
use ozmux_extension_host::{ControlExtension, HandlersBridge};
use ozmux_multiplexer::{
    AttachedSession, ExtensionActivityAid, MultiplexerCommands, SessionMarker,
};

/// The `ozmux-ext://` URL the memo extension webview is pointed at. The host
/// segment (`memo`) matches the extension name registered as a custom scheme
/// in `cef_plugin`; frames for other extension names 404.
const MEMO_WEBVIEW_URL: &str = "ozmux-ext://memo/index.html";

/// The shared asset endpoint the `ozmux-ext://` scheme reads (set on memo-ready
/// in Task 9). Empty until then, so the scheme returns 503.
#[derive(Resource, Clone, Default)]
pub struct AssetEndpoint(pub ExtensionEndpoints);

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

/// JS defining `window.ozmux` over `cef.emit` / `cef.listen`, injected per
/// webview as a `PreloadScripts` entry (see `finish_extension_setup`). Mirrors
/// `sdk/typescript/src/cef/ozmux-bridge.ts`.
pub const OZMUX_EXTENSION_JS: &str = include_str!("extension_render/ozmux.js");

/// Builds the `CefPlugin` with the `ozmux-ext://` scheme bound to the given
/// asset endpoint handle. The `window.ozmux` bridge is intentionally NOT
/// registered as a global extension here; it is injected per-webview via
/// `PreloadScripts` in `finish_extension_setup` (see the NOTE there).
pub fn cef_plugin(endpoint: &AssetEndpoint) -> CefPlugin {
    CefPlugin {
        // NOTE: "memo" is the extension-NAME segment matched inside
        // ozmux-ext://<name>/…, not the URL scheme name — the scheme is always
        // "ozmux-ext" (scheme::SCHEME_NAME). Frames for other extension names 404.
        custom_schemes: vec![custom_scheme("memo", endpoint.0.clone())],
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
                    set_asset_endpoint_once,
                    drain_handler_responses,
                    sync_focused_webview
                        .run_if(resource_exists_and_changed::<FocusedWebview>)
                        .after(OzmuxSystems::Input),
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
    hosts: Query<(Entity, &ComputedNode), (With<ExtensionActivityMarker>, Without<WebviewSource>)>,
) {
    for (host, computed) in hosts.iter() {
        let Some(logical) = pane_logical_size(computed.size(), computed.inverse_scale_factor())
        else {
            continue;
        };
        tracing::debug!(
            ?host,
            ?logical,
            url = MEMO_WEBVIEW_URL,
            "spawning extension webview"
        );
        // NOTE: `window.ozmux` MUST be a PreloadScript, not a global CefExtension.
        // ozmux.js calls cef.listen() at top level; a global extension runs that
        // during V8 context creation, where there is no entered V8 context, so the
        // native cef.listen handler's v8_context_get_current_context() crashes the
        // render process. PreloadScripts are eval'd at on_context_created inside an
        // entered context (and their exceptions are caught, not fatal), so
        // cef.listen registers correctly there.
        commands.entity(host).insert((
            WebviewSource::new(MEMO_WEBVIEW_URL),
            WebviewSize(logical),
            PreloadScripts::from([OZMUX_EXTENSION_JS]),
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
    }
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
/// targeting the emitting webview. Resolves the webview's `aid` + the (single,
/// memo) extension's handlers socket, connects idempotently, records the
/// `aid → webview` mapping for the outbound path, and forwards the frame.
///
/// Frames whose webview cannot be resolved to an `aid` are dropped — the
/// activity has not been stamped yet, so there is no handler set to address.
fn on_ozmux_frame(
    frame: On<Receive<OzmuxFrame>>,
    bridge: Res<ExtensionHandlersBridge>,
    ext: Option<Res<ControlExtension>>,
    mut aid_map: ResMut<WebviewAidMap>,
    hosts: Query<&HostActivityEntity>,
    aids: Query<&ExtensionActivityAid>,
) {
    // TODO: multi-extension — `ControlExtension` is the single memo extension,
    // so every webview routes to its one handlers socket. Resolve the socket
    // per extension once more than one is launched.
    let Some(ext) = ext else {
        return;
    };
    let webview = frame.webview;
    let Some(aid) = aid_for_webview(webview, &hosts, &aids) else {
        return;
    };
    let sock = ext.0.handlers_sock_path().to_path_buf();
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

/// Publishes the (single, memo) extension's asset socket into the
/// `ozmux-ext://` scheme's endpoint once the `ControlExtension` resource
/// exists. Idempotent via a `Local<bool>` latch; the socket binds when memo's
/// bootstrap starts, well before any webview load triggers a scheme fetch.
fn set_asset_endpoint_once(
    ext: Option<Res<ControlExtension>>,
    endpoint: Res<AssetEndpoint>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    // TODO: multi-extension — one asset endpoint is shared by the single memo
    // custom scheme; per-extension schemes need per-extension endpoints.
    if let Some(ext) = ext {
        let sock = ext.0.asset_sock_path().to_path_buf();
        tracing::debug!(asset_sock = ?sock, "published ozmux-ext asset endpoint");
        endpoint.0.set(sock);
        *done = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;

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
            .init_asset::<WebviewUiMaterial>();
        app
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
            .spawn(WebviewSource::new(MEMO_WEBVIEW_URL))
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
        let host = app
            .world_mut()
            .spawn((
                ExtensionActivityMarker,
                laid_out_node(Vec2::new(800.0, 600.0)),
            ))
            .id();
        app.update();

        let source = app
            .world()
            .get::<WebviewSource>(host)
            .expect("extension host must receive a WebviewSource");
        match source {
            WebviewSource::Url(url) => assert_eq!(url, MEMO_WEBVIEW_URL),
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
    }

    #[test]
    fn defers_webview_until_pane_is_laid_out() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app
            .world_mut()
            .spawn((ExtensionActivityMarker, ComputedNode::DEFAULT))
            .id();
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
        let host = app
            .world_mut()
            .spawn((
                ExtensionActivityMarker,
                laid_out_node(Vec2::new(800.0, 600.0)),
            ))
            .id();
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
