//! CEF integration for extension activities: the `ozmux-ext://` asset scheme and
//! the `window.ozmux` JS extension, the webview spawn-once system that attaches
//! a `bevy_cef` webview to each Extension Activity host, and the handler RPC
//! bridge (Task 9) that routes `window.ozmux` frames between the page and the
//! extension's handlers socket.

use crate::system_set::OzmuxSystems;
use crate::ui::{ExtensionActivityMarker, HostActivityEntity};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::host::ExtensionEndpoints;
use ozmux_extension_host::scheme::custom_scheme;
use ozmux_extension_host::{ControlExtension, HandlersBridge};
use ozmux_multiplexer::ExtensionActivityAid;

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

/// JS defining `window.ozmux` over `cef.emit` / `cef.listen`, injected as a
/// global CEF extension. Mirrors `sdk/typescript/src/cef/ozmux-bridge.ts`.
pub const OZMUX_EXTENSION_JS: &str = include_str!("extension_render/ozmux.js");

/// Builds the `CefPlugin` with the `ozmux-ext://` scheme + `window.ozmux`
/// extension, bound to the given asset endpoint handle.
pub fn cef_plugin(endpoint: &AssetEndpoint) -> CefPlugin {
    CefPlugin {
        // NOTE: "memo" is the extension-NAME segment matched inside
        // ozmux-ext://<name>/…, not the URL scheme name — the scheme is always
        // "ozmux-ext" (scheme::SCHEME_NAME). Frames for other extension names 404.
        custom_schemes: vec![custom_scheme("memo", endpoint.0.clone())],
        extensions: CefExtensions::new().add("ozmux", OZMUX_EXTENSION_JS),
        ..Default::default()
    }
}

/// Wires the spawn-once system that attaches a `bevy_cef` webview to each
/// Extension Activity host.
///
/// Resize is intentionally NOT handled here: `bevy_cef`'s `UiWebviewPlugin`
/// (pulled in by `CefPlugin`) already runs `update_webview_ui_size` in
/// `PostUpdate` after `UiSystems::Layout`, syncing each UI webview's
/// `WebviewSize` from its `ComputedNode`. Adding a second writer would thrash
/// the same component. The terminal path needs its own resize only because it
/// derives grid `cols`/`rows` from font metrics, which `bevy_cef` knows nothing
/// about.
pub struct OzmuxExtensionRenderPlugin;

impl Plugin for OzmuxExtensionRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JsEmitEventPlugin::<OzmuxFrame>::default())
            .init_resource::<ExtensionHandlersBridge>()
            .init_resource::<WebviewAidMap>()
            .add_observer(on_ozmux_frame)
            .add_systems(
                Update,
                (
                    finish_extension_setup.in_set(OzmuxSystems::SetupActivity),
                    set_asset_endpoint_once,
                    drain_handler_responses,
                ),
            );
    }
}

/// Attaches a `bevy_cef` webview to each freshly-spawned Extension Activity
/// host: a `WebviewSource` pointed at the memo extension plus a
/// `MaterialNode<WebviewUiMaterial>`. Runs every Update tick but only targets
/// hosts that lack `WebviewSource`, so the per-entity insertion happens
/// exactly once.
///
/// The host already carries a full-size `Node` (`width`/`height: 100%`) from
/// `build_activity_host_children`, so the webview fills its pane; `bevy_cef`'s
/// `update_webview_ui_size` keeps `WebviewSize` in step with the node's
/// `ComputedNode` extents on every layout pass.
fn finish_extension_setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<WebviewUiMaterial>>,
    hosts: Query<Entity, (With<ExtensionActivityMarker>, Without<WebviewSource>)>,
) {
    for host in hosts.iter() {
        commands.entity(host).insert((
            WebviewSource::new(MEMO_WEBVIEW_URL),
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
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
        endpoint.0.set(ext.0.asset_sock_path().to_path_buf());
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
    fn skips_entities_without_extension_marker() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app.world_mut().spawn_empty().id();
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
        let host = app.world_mut().spawn(ExtensionActivityMarker).id();
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
    }

    #[test]
    fn webview_inserted_exactly_once() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app.world_mut().spawn(ExtensionActivityMarker).id();
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
