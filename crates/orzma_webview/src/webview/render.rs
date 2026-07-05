//! CEF webview wiring for the `window.orzma` Tier 1 back-channel: registers the
//! `orzma://` dynamic asset scheme and routes the `orzma.call` frames the page bridge
//! emits to the registering program over the control socket.

use crate::control_plane::{ConnectionWriters, OrzmaRpc, WebviewOwner};
use bevy::prelude::*;
use bevy_cef::prelude::*;
use orzma_webview_host::WebviewAssetRegistry;
use orzma_webview_host::orzma_scheme::custom_orzma_scheme;
use serde_json::Value;
use std::path::Path;

pub(crate) mod preload;

/// One frame emitted by the page bridge (`orzma_bridge.js`) via
/// `cef.emit({ kind: '…', … })`, inspected by the per-kind observers.
///
/// `#[serde(transparent)]` makes it deserialize from the bare emitted object
/// (`{kind, reqId, …}`), not from a `{"0": …}` wrapper — `bevy_cef`'s
/// `cef.emit(frame)` serializes only its first argument into one global
/// `Receive<OrzmaFrame>`.
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
struct OrzmaFrame(serde_json::Value);

/// The `kind` discriminator routing a `Receive<OrzmaFrame>` to the Tier 1
/// back-channel (`on_orzma_call_frame`). The page side emits it in `orzma_bridge.js`.
const ORZMA_CALL_KIND: &str = "orzma.call";

/// The `kind` discriminator routing a `Receive<OrzmaFrame>` to the one-way
/// inbound-event forwarder (`on_orzma_emit_frame`). Emitted by `orzma_bridge.js`.
const ORZMA_EMIT_KIND: &str = "orzma.emit";

/// Builds the `CefPlugin` with the `orzma://` (dynamic, Tier 1) scheme bound
/// to its shared `WebviewAssetRegistry`, using `root_cache_path` as this process's
/// unique CEF profile directory (one Chromium singleton lock per instance).
pub fn cef_plugin(orzma_registry: WebviewAssetRegistry, root_cache_path: &Path) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_orzma_scheme(orzma_registry)],
        command_line_config: cef_command_line_config(),
        root_cache_path: Some(root_cache_path.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

/// CEF command-line switches for the embedded webview.
///
/// On macOS we always append `use-mock-keychain` so CEF's OSCrypt layer derives
/// its cookie / Local State encryption key from a mock keychain rather than the
/// real login keychain. Without it, release / bundled builds pop the "orzma
/// wants to use … Chromium Safe Storage" keychain prompt on launch:
/// `bevy_cef_core`'s `CommandLineConfig::default()` only carries this switch under
/// `debug_assertions`, so it is absent from the `dist` profile. orzma's CEF
/// profile is an ephemeral per-process temp dir (see `cef_profile`), so a mock
/// key costs no real persistence. `effective_command_line_config` de-duplicates,
/// so re-adding it on debug builds is harmless.
///
/// The `debug` feature additionally exposes `remote-debugging-port` — a local
/// Chromium DevTools (CDP) endpoint on `127.0.0.1:9222` for inspecting the
/// embedded webview — and is off by default so that endpoint is never exposed in
/// normal builds.
fn cef_command_line_config() -> CommandLineConfig {
    let config = CommandLineConfig::default();
    #[cfg(target_os = "macos")]
    let config = config.with_switch("use-mock-keychain");
    #[cfg(feature = "debug")]
    let config = config.with_switch_value("remote-debugging-port", "9222");
    config
}

/// Wires the `window.orzma` Tier 1 back-channel: the `orzma.call` frame
/// observer, the webview-load loggers, and the focus sync that keeps
/// `bevy_cef`'s `FocusedWebview` in step with the active pane.
pub(crate) struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JsEmitEventPlugin::<OrzmaFrame>::default())
            .add_observer(on_orzma_call_frame)
            .add_observer(on_orzma_emit_frame)
            .add_observer(on_webview_address_changed)
            .add_observer(drop_orzma_inflight_on_webview_despawn)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error);
    }
}

/// Inbound (Tier 1 back-channel): a `window.orzma.call` arrives as a
/// `Receive<OrzmaFrame>` with `kind:"orzma.call"`. The trusted caller is
/// `frame.webview` (bound per-webview by `bevy_cef`, never the JS payload); its
/// `WebviewOwner` names the registering connection. The call is forwarded over
/// that connection's writer under a Rust-minted global reqId; a missing
/// owner/connection rejects the page Promise directly.
///
/// Registered as an observer on the shared `Receive<OrzmaFrame>` event (NOT a
/// second `JsEmitEventPlugin`): the event carries all frames; non-`orzma.call`
/// frames are ignored via the early return on `ORZMA_CALL_KIND`.
fn on_orzma_call_frame(
    frame: On<Receive<OrzmaFrame>>,
    mut commands: Commands,
    mut rpc: ResMut<OrzmaRpc>,
    writers: Res<ConnectionWriters>,
    owners: Query<&WebviewOwner>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(ORZMA_CALL_KIND) {
        return;
    }
    let webview = frame.webview;
    let req_id = payload
        .get("reqId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = payload.get("params").cloned().unwrap_or(Value::Null);

    let Ok(owner) = owners.get(webview) else {
        reject_orzma_call(&mut commands, webview, req_id, "no_owner");
        return;
    };
    let global_id = rpc.mint();
    let line = serde_json::json!({
        "op": "call", "handle": owner.handle, "reqId": global_id, "method": method, "params": params
    })
    .to_string();
    if !writers.send(owner.connection_id, line) {
        reject_orzma_call(&mut commands, webview, req_id, "owner_unavailable");
        return;
    }
    rpc.note(&global_id, webview, req_id, owner.connection_id);
}

/// Emits a `{reqId, ok:false, error}` reply to one webview on the `"orzma"`
/// channel (settling the page Promise).
fn reject_orzma_call(commands: &mut Commands, webview: Entity, req_id: &str, error: &str) {
    let payload = serde_json::json!({ "reqId": req_id, "ok": false, "error": error });
    commands.trigger(HostEmitEvent::new(webview, "orzma", &payload));
}

/// Inbound (one-way): a `window.orzma.emit` arrives as a `Receive<OrzmaFrame>`
/// with `kind:"orzma.emit"`. The trusted caller is `frame.webview` (bound per
/// webview by `bevy_cef`); its `WebviewOwner` names the registering connection.
/// The event is forwarded as a fire-and-forget `{op:"event"}` line — no reqId,
/// no reply, no `OrzmaRpc` tracking. A missing owner or unavailable connection
/// drops the event (debug-logged); there is no page Promise to settle.
///
/// Registered on the shared `Receive<OrzmaFrame>` event (not a second
/// `JsEmitEventPlugin`); frames whose `kind` is not `orzma.emit` are ignored.
fn on_orzma_emit_frame(
    frame: On<Receive<OrzmaFrame>>,
    writers: Res<ConnectionWriters>,
    owners: Query<&WebviewOwner>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(ORZMA_EMIT_KIND) {
        return;
    }
    let event = payload
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if event.is_empty() {
        tracing::debug!("orzma.emit frame with an empty event name; dropping");
        return;
    }

    let Ok(owner) = owners.get(frame.webview) else {
        tracing::debug!("orzma.emit frame for a webview with no owner; dropping");
        return;
    };
    let body = payload.get("payload").cloned().unwrap_or(Value::Null);
    let line = serde_json::json!({
        "op": "event", "handle": owner.handle, "event": event, "payload": body
    })
    .to_string();
    if !writers.send(owner.connection_id, line) {
        tracing::debug!(
            handle = owner.handle,
            "orzma.emit owner connection unavailable; dropping"
        );
    }
}

/// Outbound (Tier 1 back-channel): when a webview's top-level URL changes (CEF
/// `OnAddressChange` — link clicks, hint activations, redirects, hash/pushState),
/// forwards a `urlChanged` call to the registering program so it can track
/// page-driven navigation (e.g. orzbrowser's history + URL bar). Scoped to remote
/// `http(s)` webviews; `orzma://` dir/inline views (which register no
/// `urlChanged` handler) are skipped. Fire-and-forget: the minted reqId is not
/// recorded, so the program's reply finds no in-flight entry and is dropped by
/// `OrzmaRpc::take_for_connection`.
fn on_webview_address_changed(
    addr: On<AddressChanged>,
    mut rpc: ResMut<OrzmaRpc>,
    writers: Res<ConnectionWriters>,
    views: Query<(&WebviewOwner, &WebviewSource)>,
) {
    let Ok((owner, source)) = views.get(addr.webview) else {
        return;
    };
    let WebviewSource::Url(source_url) = source else {
        return;
    };
    if !(source_url.starts_with("http://") || source_url.starts_with("https://")) {
        return;
    }
    let line = serde_json::json!({
        "op": "call",
        "handle": owner.handle,
        "reqId": rpc.mint(),
        "method": "urlChanged",
        "params": { "url": addr.url },
    })
    .to_string();
    let _ = writers.send(owner.connection_id, line);
}

/// Despawn prune: drop a despawned webview's in-flight back-channel calls.
fn drop_orzma_inflight_on_webview_despawn(
    remove: On<Remove, WebviewOwner>,
    mut rpc: ResMut<OrzmaRpc>,
) {
    rpc.drain_webview(remove.entity);
}

/// Logs the start of a webview page load. Debug-level diagnostics: these
/// observers fire for every `bevy_cef` webview, not only orzma webviews.
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

    #[test]
    fn orzma_frame_deserializes_from_bare_emitted_object() {
        let raw = r#"{"kind":"call","id":"c0","name":"greet","payload":{"x":1}}"#;
        let frame: OrzmaFrame = serde_json::from_str(raw).expect("transparent newtype");
        assert_eq!(frame.0["kind"], "call");
        assert_eq!(frame.0["id"], "c0");
        assert_eq!(frame.0["name"], "greet");
        assert_eq!(frame.0["payload"]["x"], 1);
    }

    #[test]
    fn orzma_emit_frame_pushes_event_to_owner_connection() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_orzma_emit_frame);

        let webview = app
            .world_mut()
            .spawn(WebviewOwner {
                connection_id: 7,
                handle: "H".into(),
            })
            .id();

        app.world_mut().trigger(Receive {
            webview,
            payload: OrzmaFrame(serde_json::json!({
                "kind": "orzma.emit", "event": "hello", "payload": {"message": "hi"}
            })),
        });

        let line = rx.try_recv().expect("an event was pushed");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "event");
        assert_eq!(v["handle"], "H");
        assert_eq!(v["event"], "hello");
        assert_eq!(v["payload"]["message"], "hi");
    }

    #[test]
    fn orzma_emit_frame_without_owner_is_dropped() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_orzma_emit_frame);

        // A webview entity with no WebviewOwner component.
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(Receive {
            webview,
            payload: OrzmaFrame(serde_json::json!({
                "kind": "orzma.emit", "event": "hello", "payload": null
            })),
        });

        assert!(rx.try_recv().is_err(), "no owner ⇒ nothing forwarded");
    }

    #[test]
    fn orzma_emit_frame_with_empty_event_is_dropped() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_orzma_emit_frame);

        let webview = app
            .world_mut()
            .spawn(WebviewOwner {
                connection_id: 7,
                handle: "H".into(),
            })
            .id();
        app.world_mut().trigger(Receive {
            webview,
            payload: OrzmaFrame(serde_json::json!({
                "kind": "orzma.emit", "event": "", "payload": {"message": "hi"}
            })),
        });

        assert!(
            rx.try_recv().is_err(),
            "an empty event name must be dropped, not forwarded"
        );
    }

    #[test]
    fn orzma_call_frame_pushes_call_to_owner_connection() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(OrzmaRpc::default());
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_orzma_call_frame);

        let webview = app
            .world_mut()
            .spawn(WebviewOwner {
                connection_id: 7,
                handle: "H".into(),
            })
            .id();

        app.world_mut().trigger(Receive {
            webview,
            payload: OrzmaFrame(serde_json::json!({
                "kind": "orzma.call", "reqId": "p0", "method": "save", "params": [1, 2]
            })),
        });

        let line = rx.try_recv().expect("a call was pushed");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "call");
        assert_eq!(v["handle"], "H");
        assert_eq!(v["method"], "save");
        assert_eq!(v["reqId"], "0");
        assert_eq!(v["params"], serde_json::json!([1, 2]));
        assert_eq!(
            app.world()
                .resource::<OrzmaRpc>()
                .count_in_flight_for_test(),
            1
        );
    }

    fn address_changed_app() -> (App, crossbeam_channel::Receiver<String>) {
        use crossbeam_channel::unbounded;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(OrzmaRpc::default());
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_webview_address_changed);
        (app, rx)
    }

    #[test]
    fn address_change_pushes_urlchanged_call_to_owner_for_http_url() {
        let (mut app, rx) = address_changed_app();
        let webview = app
            .world_mut()
            .spawn((
                WebviewOwner {
                    connection_id: 7,
                    handle: "H".into(),
                },
                WebviewSource::new("https://example.com"),
            ))
            .id();

        app.world_mut().trigger(AddressChanged {
            webview,
            url: "https://example.com/next".into(),
            can_go_back: true,
            can_go_forward: false,
        });

        let line = rx.try_recv().expect("a urlChanged call was pushed");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "call");
        assert_eq!(v["handle"], "H");
        assert_eq!(v["method"], "urlChanged");
        assert_eq!(v["params"]["url"], "https://example.com/next");
        assert_eq!(
            app.world()
                .resource::<OrzmaRpc>()
                .count_in_flight_for_test(),
            0,
            "urlChanged is fire-and-forget: it records no in-flight call"
        );
    }

    #[test]
    fn address_change_on_dyn_view_pushes_nothing() {
        let (mut app, rx) = address_changed_app();
        let webview = app
            .world_mut()
            .spawn((
                WebviewOwner {
                    connection_id: 7,
                    handle: "H".into(),
                },
                WebviewSource::new("orzma://H/index.html"),
            ))
            .id();

        app.world_mut().trigger(AddressChanged {
            webview,
            url: "orzma://H/index.html#section".into(),
            can_go_back: false,
            can_go_forward: false,
        });

        assert!(
            rx.try_recv().is_err(),
            "an orzma:// dir/inline view must report no urlChanged"
        );
    }
}
