//! CEF wiring for the `window.orzma` page bridge: registers the `orzma://`
//! scheme, reports page frames and address changes to the webview host, and
//! applies the host's replies, events, and navigations to pages.

use crate::webview::mount::{Bridged, Webview, webview_of_mount};
use crate::webview::scheme::{WebviewAssetRegistry, custom_orzma_scheme};
use bevy::prelude::*;
use bevy_cef::prelude::{
    AddressChanged, CefPlugin, CommandLineConfig, HostEmitEvent, JsEmitEventPlugin, LoadError,
    LoadFinished, LoadStarted, Receive, RequestGoBack, RequestGoForward, RequestNavigate,
    RequestReload, WebviewSource,
};
use bevy_orzmux::prelude::{OrzmuxConnection, OrzmuxWebviewEvent};
use orzma_webview_host::prelude::{Navigation, PageOutcome, WebviewCommand, WebviewEvent};
use orzmux::prelude::OrzmuxCommand;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) mod preload;

/// Builds the `CefPlugin` with the `orzma://` (dynamic, Tier 1) scheme bound
/// to its shared `WebviewAssetRegistry`, using `root_cache_path` as this
/// process's unique CEF profile directory.
pub fn cef_plugin(orzma_registry: WebviewAssetRegistry, root_cache_path: &Path) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_orzma_scheme(orzma_registry)],
        command_line_config: cef_command_line_config(),
        root_cache_path: Some(root_cache_path.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

/// Wires the `window.orzma` page bridge: the `orzma.call` and `orzma.emit`
/// frame observers, the address-change reporter and tracker, the observers
/// that apply the host's replies, events, and navigations to pages, and the
/// page-load loggers.
pub(crate) struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JsEmitEventPlugin::<OrzmaFrame>::default())
            .add_observer(on_orzma_call_frame)
            .add_observer(on_orzma_emit_frame.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(on_webview_address_changed.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(track_page_address)
            .add_observer(deliver_to_page)
            .add_observer(apply_navigation)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error);
    }
}

/// One frame emitted by the page bridge (`orzma_bridge.js`) via
/// `cef.emit({ kind: '…', … })`.
///
/// It deserializes from the bare emitted object (`{kind, reqId, …}`), not
/// from a `{"0": …}` wrapper.
#[derive(Deserialize, Clone, Debug)]
#[serde(transparent)]
struct OrzmaFrame(Value);

/// The top-level URL an orzma webview last reported through
/// `AddressChanged` or was last sent to by a `Navigate`, whichever came
/// later.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct PageAddress(String);

/// The `kind` discriminator of a page's `window.orzma.call` frame, emitted
/// by `orzma_bridge.js`.
const ORZMA_CALL_KIND: &str = "orzma.call";

/// The `kind` discriminator of a page's `window.orzma.emit` frame, emitted
/// by `orzma_bridge.js`.
const ORZMA_EMIT_KIND: &str = "orzma.emit";

/// CEF command-line switches for the embedded webview.
///
/// On macOS the config always carries `use-mock-keychain`, so CEF's OSCrypt
/// layer derives its cookie and Local State encryption key from a mock
/// keychain rather than the real login keychain.
///
/// On Windows the config always carries `disable-gpu-compositing`, so the
/// embedded pages composite on the CPU rather than on the GPU.
///
/// The `debug` feature additionally exposes `remote-debugging-port`, a local
/// Chromium DevTools (CDP) endpoint on `127.0.0.1:9222` for inspecting the
/// embedded webview. It is off by default.
fn cef_command_line_config() -> CommandLineConfig {
    let config = CommandLineConfig::default();
    #[cfg(target_os = "macos")]
    let config = config.with_switch("use-mock-keychain");
    // NOTE: CEF wedges an off-screen browser permanently when the frame for a
    // resize never arrives (chromiumembedded/cef#3826). The Viz capture oracle stops
    // completing captures, so `hold_resize_` is never released and every later
    // `WasResized` becomes a no-op. Software compositing never creates the video
    // consumer, so `InvalidateInternal` paints synchronously and that oracle is out
    // of the picture. The upstream fix landed on CEF branch 8037, which is later than
    // the pinned CEF 152 (Chromium build 7977), so this switch must stay until the
    // pinned CEF carries it.
    #[cfg(target_os = "windows")]
    let config = config.with_switch("disable-gpu-compositing");
    #[cfg(feature = "debug")]
    let config = config.with_switch_value("remote-debugging-port", "9222");
    config
}

/// Reports a page's `window.orzma.call` (a `Receive<OrzmaFrame>` with
/// `kind:"orzma.call"`) to the host as a `PageCall` of the webview's mount;
/// any other `kind` is ignored.
///
/// The caller is the frame's webview, never the payload. A frame from a
/// webview that is not a bridged orzma mount is rejected with `no_owner`,
/// and one that arrives after the multiplexer is gone with
/// `owner_unavailable`, both settling the page's promise at once.
fn on_orzma_call_frame(
    frame: On<Receive<OrzmaFrame>>,
    mut commands: Commands,
    connection: Option<Res<OrzmuxConnection>>,
    webviews: Query<&Webview, With<Bridged>>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(ORZMA_CALL_KIND) {
        return;
    }
    let webview = frame.webview;
    let page_req = payload
        .get("reqId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Ok(view) = webviews.get(webview) else {
        reject_orzma_call(&mut commands, webview, page_req, "no_owner");
        return;
    };
    let Some(connection) = connection else {
        reject_orzma_call(&mut commands, webview, page_req, "owner_unavailable");
        return;
    };
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    connection
        .0
        .send(OrzmuxCommand::Webview(WebviewCommand::PageCall {
            mount: view.mount(),
            page_req: page_req.to_string(),
            method: method.to_string(),
            params: payload.get("params").cloned().unwrap_or(Value::Null),
        }));
    if connection.0.is_disconnected() {
        reject_orzma_call(&mut commands, webview, page_req, "owner_unavailable");
    }
}

/// Reports a page's `window.orzma.emit` (a `Receive<OrzmaFrame>` with
/// `kind:"orzma.emit"`) to the host as a `PageEmit` of the webview's mount;
/// any other `kind` is ignored. An emit with an empty event name, or from a
/// webview that is not a bridged orzma mount, is dropped.
fn on_orzma_emit_frame(
    frame: On<Receive<OrzmaFrame>>,
    connection: Res<OrzmuxConnection>,
    webviews: Query<&Webview, With<Bridged>>,
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
    let Ok(view) = webviews.get(frame.webview) else {
        tracing::debug!(
            "orzma.emit frame from a webview that is not a bridged orzma mount; dropping"
        );
        return;
    };
    connection
        .0
        .send(OrzmuxCommand::Webview(WebviewCommand::PageEmit {
            mount: view.mount(),
            event: event.to_string(),
            payload: payload.get("payload").cloned().unwrap_or(Value::Null),
        }));
}

/// Reports a bridged remote webview's new top-level URL (CEF
/// `OnAddressChange`: link clicks, redirects, hash and pushState navigation)
/// to the host as a `UrlChanged` of its mount. Any other webview, including
/// an `orzma://` page, is ignored.
fn on_webview_address_changed(
    addr: On<AddressChanged>,
    connection: Res<OrzmuxConnection>,
    webviews: Query<(&Webview, &WebviewSource), With<Bridged>>,
) {
    let Ok((view, source)) = webviews.get(addr.webview) else {
        return;
    };
    let remote = matches!(
        source,
        WebviewSource::Url(url) if url.starts_with("http://") || url.starts_with("https://")
    );
    if !remote {
        return;
    }
    connection
        .0
        .send(OrzmuxCommand::Webview(WebviewCommand::UrlChanged {
            mount: view.mount(),
            url: addr.url.clone(),
        }));
}

/// Settles a page's call with a `PageReply`, or delivers a program's `emit`
/// with a `PageEvent`, on the page of the mount the event names: the reply on
/// the `"orzma"` channel as `{reqId, ok, value | error}`, the event on
/// `"orzma.event"` as `{event, payload}`. A mount with no live webview is
/// ignored.
fn deliver_to_page(
    ev: On<OrzmuxWebviewEvent>,
    mut commands: Commands,
    webviews: Query<(Entity, &Webview)>,
) {
    let (mount, channel, payload) = match ev.webview_event() {
        WebviewEvent::PageReply {
            mount,
            page_req,
            outcome,
        } => (*mount, "orzma", reply_payload(page_req, outcome)),
        WebviewEvent::PageEvent {
            mount,
            event,
            payload,
        } => (
            *mount,
            "orzma.event",
            json!({ "event": event, "payload": payload }),
        ),
        _ => return,
    };
    let Some(webview) = webview_of_mount(&webviews, mount) else {
        tracing::debug!(?mount, "page message for a mount with no webview dropped");
        return;
    };
    commands.trigger(HostEmitEvent::new(webview, channel, &payload));
}

/// Records the top-level URL an orzma webview reports in its `PageAddress`;
/// any other webview is ignored.
fn track_page_address(
    addr: On<AddressChanged>,
    mut commands: Commands,
    webviews: Query<(), With<Webview>>,
) {
    if webviews.contains(addr.webview) {
        commands
            .entity(addr.webview)
            .try_insert(PageAddress(addr.url.clone()));
    }
}

/// Applies a `Navigate` to the webview of its mount, and `Back`, `Forward`,
/// and `Reload` ask CEF. A mount with no live webview is ignored.
///
/// `To` does nothing when the page already shows or is loading the URL: the
/// address it last reported or was last sent to, or its `WebviewSource`
/// before either. Otherwise it replaces the `WebviewSource` with a URL that
/// differs from it, asks CEF to load one that equals it, and records the URL
/// as the page's address.
fn apply_navigation(
    ev: On<OrzmuxWebviewEvent>,
    mut commands: Commands,
    mut webviews: Query<(Entity, &Webview, &mut WebviewSource, Option<&PageAddress>)>,
) {
    let WebviewEvent::Navigate { mount, navigation } = ev.webview_event() else {
        return;
    };
    let Some((webview, _, mut source, address)) = webviews
        .iter_mut()
        .find(|(_, view, _, _)| view.mount() == *mount)
    else {
        tracing::debug!(?mount, "navigation for a mount with no webview dropped");
        return;
    };
    match navigation {
        Navigation::To(url) => {
            let loaded = matches!(&*source, WebviewSource::Url(current) if current == url);
            let showing = address.map_or(loaded, |address| address.0 == *url);
            if showing {
                return;
            }
            if loaded {
                commands.trigger(RequestNavigate {
                    webview,
                    url: url.clone(),
                });
            } else {
                *source = WebviewSource::Url(url.clone());
            }
            commands
                .entity(webview)
                .try_insert(PageAddress(url.clone()));
        }
        Navigation::Back => commands.trigger(RequestGoBack { webview }),
        Navigation::Forward => commands.trigger(RequestGoForward { webview }),
        Navigation::Reload => commands.trigger(RequestReload { webview }),
    }
}

/// Settles one page promise with `{reqId, ok:false, error}` on the `"orzma"`
/// channel.
fn reject_orzma_call(commands: &mut Commands, webview: Entity, page_req: &str, error: &str) {
    let payload = json!({ "reqId": page_req, "ok": false, "error": error });
    commands.trigger(HostEmitEvent::new(webview, "orzma", &payload));
}

/// The `"orzma"` channel payload that settles the page's call `page_req`.
fn reply_payload(page_req: &str, outcome: &PageOutcome) -> Value {
    match outcome {
        Ok(value) => json!({ "reqId": page_req, "ok": true, "value": value }),
        Err(error) => json!({ "reqId": page_req, "ok": false, "error": error }),
    }
}

/// Logs the start of a webview page load at debug level.
///
/// It fires for every `bevy_cef` webview, not only orzma webviews.
fn log_webview_load_started(load: On<LoadStarted>) {
    tracing::debug!(webview = ?load.webview, "webview load started");
}

/// Logs a finished page load and its HTTP status.
fn log_webview_load_finished(load: On<LoadFinished>) {
    tracing::debug!(
        webview = ?load.webview,
        status = load.http_status_code,
        "webview load finished"
    );
}

/// Logs a page load failure (CEF `OnLoadError`) at `warn` level.
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
    use bevy_orzmux::prelude::OrzmuxClient;
    use crossbeam_channel::Receiver;
    use orzma_vt::prelude::InstanceId;
    use orzma_webview_host::prelude::{HandleId, MountId};
    use orzmux::prelude::CommandSeq;

    /// The `HostEmitEvent`s the page bridge sent: webview, channel, payload.
    #[derive(Resource, Default)]
    struct Emitted(Vec<(Entity, String, Value)>);

    fn app() -> (App, Receiver<(CommandSeq, OrzmuxCommand)>) {
        let (client, _events, commands) = OrzmuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Emitted>()
            .insert_resource(OrzmuxConnection(client))
            .add_observer(on_orzma_call_frame)
            .add_observer(on_orzma_emit_frame.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(on_webview_address_changed.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(track_page_address)
            .add_observer(deliver_to_page)
            .add_observer(apply_navigation)
            .add_observer(|ev: On<HostEmitEvent>, mut emitted: ResMut<Emitted>| {
                let payload = serde_json::from_str(&ev.payload).unwrap_or(Value::Null);
                emitted.0.push((ev.webview, ev.id.clone(), payload));
            });
        (app, commands)
    }

    /// Spawns the webview of `mount` loading `url`, with the bridge.
    fn spawn_mounted(app: &mut App, mount: MountId, url: &str) -> Entity {
        let webview = spawn_display_only(app, mount, url);
        app.world_mut().entity_mut(webview).insert(Bridged);
        webview
    }

    /// Spawns the webview of `mount` loading `url`, without the bridge.
    fn spawn_display_only(app: &mut App, mount: MountId, url: &str) -> Entity {
        app.world_mut()
            .spawn((
                Webview::new(HandleId::from("H"), InstanceId(1), mount, 0, 10, 40),
                WebviewSource::new(url),
            ))
            .id()
    }

    fn frame(app: &mut App, webview: Entity, payload: Value) {
        app.world_mut().trigger(Receive {
            webview,
            payload: OrzmaFrame(payload),
        });
        app.world_mut().flush();
    }

    fn host_event(app: &mut App, event: WebviewEvent<Entity>) {
        app.world_mut()
            .trigger(OrzmuxWebviewEvent::new(event, CommandSeq(0)));
        app.world_mut().flush();
    }

    fn sent(commands: &Receiver<(CommandSeq, OrzmuxCommand)>) -> Vec<WebviewCommand> {
        commands
            .try_iter()
            .filter_map(|(_, command)| match command {
                OrzmuxCommand::Webview(command) => Some(command),
                _ => None,
            })
            .collect()
    }

    /// Asserts that a bridge frame deserializes from the bare object the page
    /// emits, not from a wrapper around it.
    ///
    /// Case: a page calls `window.orzma.call("greet", {x: 1})` and the bridge
    /// emits its frame.
    #[test]
    fn orzma_frame_deserializes_from_bare_emitted_object() {
        let raw = r#"{"kind":"orzma.call","reqId":"o0","method":"greet","params":{"x":1}}"#;
        let frame: OrzmaFrame = serde_json::from_str(raw).expect("transparent newtype");
        assert_eq!(frame.0["kind"], ORZMA_CALL_KIND);
        assert_eq!(frame.0["reqId"], "o0");
        assert_eq!(frame.0["method"], "greet");
        assert_eq!(frame.0["params"]["x"], 1);
    }

    /// Asserts that a page's `window.orzma.call` reaches the host as a
    /// `PageCall` of the webview's mount, carrying the page's own id.
    ///
    /// Case: a markdown page asks its program to save the document.
    #[test]
    fn a_page_call_is_reported_for_its_mount() {
        let (mut app, commands) = app();
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        frame(
            &mut app,
            webview,
            json!({"kind": "orzma.call", "reqId": "p0", "method": "save", "params": [1, 2]}),
        );
        assert_eq!(
            sent(&commands),
            vec![WebviewCommand::PageCall {
                mount: MountId::new(3),
                page_req: "p0".into(),
                method: "save".into(),
                params: json!([1, 2]),
            }]
        );
    }

    /// Asserts that a page without the bridge reaches the host with none of
    /// its frames or address changes, its call rejected at once with
    /// `no_owner`, and that an `orzma://` page's address change is not
    /// reported either.
    ///
    /// Case: an untrusted remote site shown read-only sends forged bridge
    /// frames through `cef.emit` and navigates, while a bundled page changes
    /// its hash.
    #[test]
    fn frames_and_address_changes_of_pages_without_the_bridge_stay_local() {
        let (mut app, commands) = app();
        let remote = spawn_display_only(&mut app, MountId::new(3), "https://example.com/");
        frame(
            &mut app,
            remote,
            json!({"kind": "orzma.call", "reqId": "p0", "method": "save"}),
        );
        frame(
            &mut app,
            remote,
            json!({"kind": "orzma.emit", "event": "tick"}),
        );
        let bundled = spawn_mounted(&mut app, MountId::new(4), "orzma://H/index.html");
        for (webview, url) in [
            (remote, "https://example.com/next"),
            (bundled, "orzma://H/index.html#top"),
        ] {
            app.world_mut().trigger(AddressChanged {
                webview,
                url: url.into(),
                can_go_back: false,
                can_go_forward: false,
            });
        }
        app.world_mut().flush();
        assert!(sent(&commands).is_empty());
        assert_eq!(
            app.world().resource::<Emitted>().0,
            vec![(
                remote,
                "orzma".to_string(),
                json!({"reqId": "p0", "ok": false, "error": "no_owner"}),
            )]
        );
    }

    /// Asserts that a call from a webview that is not an orzma mount is
    /// rejected on the page with `no_owner` and never reaches the host.
    ///
    /// Case: a page loaded outside orzma's mount flow calls
    /// `window.orzma.call`.
    #[test]
    fn a_call_from_a_webview_without_a_mount_is_rejected_at_once() {
        let (mut app, commands) = app();
        let webview = app.world_mut().spawn_empty().id();
        frame(
            &mut app,
            webview,
            json!({"kind": "orzma.call", "reqId": "p0", "method": "save"}),
        );
        assert!(sent(&commands).is_empty());
        assert_eq!(
            app.world().resource::<Emitted>().0,
            vec![(
                webview,
                "orzma".to_string(),
                json!({"reqId": "p0", "ok": false, "error": "no_owner"}),
            )]
        );
    }

    /// Asserts that a call made after the multiplexer is gone is rejected on
    /// the page with `owner_unavailable`.
    ///
    /// Case: the backend thread died and a page calls its program before the
    /// app exits.
    #[test]
    fn a_call_without_the_multiplexer_is_rejected_as_unavailable() {
        let (mut app, _commands) = app();
        app.world_mut().remove_resource::<OrzmuxConnection>();
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        frame(
            &mut app,
            webview,
            json!({"kind": "orzma.call", "reqId": "p0", "method": "save"}),
        );
        assert_eq!(
            app.world().resource::<Emitted>().0,
            vec![(
                webview,
                "orzma".to_string(),
                json!({"reqId": "p0", "ok": false, "error": "owner_unavailable"}),
            )]
        );
    }

    /// Asserts that a call sent after the multiplexer thread stopped is
    /// rejected on the page with `owner_unavailable`.
    ///
    /// Case: the backend thread died a moment ago, and a page calls its
    /// program before the drain removes the connection.
    #[test]
    fn a_call_after_the_multiplexer_stopped_is_rejected_as_unavailable() {
        let (mut app, commands) = app();
        drop(commands);
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        frame(
            &mut app,
            webview,
            json!({"kind": "orzma.call", "reqId": "p0", "method": "save"}),
        );
        assert_eq!(
            app.world().resource::<Emitted>().0,
            vec![(
                webview,
                "orzma".to_string(),
                json!({"reqId": "p0", "ok": false, "error": "owner_unavailable"}),
            )]
        );
    }

    /// Asserts that a page's `window.orzma.emit` reaches the host as a
    /// `PageEmit` of the webview's mount.
    ///
    /// Case: a page tells its program that the user scrolled to a heading.
    #[test]
    fn a_page_emit_is_reported_for_its_mount() {
        let (mut app, commands) = app();
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        frame(
            &mut app,
            webview,
            json!({"kind": "orzma.emit", "event": "hello", "payload": {"message": "hi"}}),
        );
        assert_eq!(
            sent(&commands),
            vec![WebviewCommand::PageEmit {
                mount: MountId::new(3),
                event: "hello".into(),
                payload: json!({"message": "hi"}),
            }]
        );
    }

    /// Asserts that an emit with an empty event name never reaches the host.
    ///
    /// Case: a page calls `window.orzma.emit("")` by mistake.
    #[test]
    fn an_emit_with_an_empty_event_name_is_dropped() {
        let (mut app, commands) = app();
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        frame(
            &mut app,
            webview,
            json!({"kind": "orzma.emit", "event": "", "payload": null}),
        );
        assert!(sent(&commands).is_empty());
    }

    /// Asserts that a webview's address change reaches the host as a
    /// `UrlChanged` of its mount.
    ///
    /// Case: the user follows a link inside a remote page a TUI browser
    /// shows.
    #[test]
    fn an_address_change_is_reported_for_its_mount() {
        let (mut app, commands) = app();
        let webview = spawn_mounted(&mut app, MountId::new(3), "https://example.com");
        app.world_mut().trigger(AddressChanged {
            webview,
            url: "https://example.com/next".into(),
            can_go_back: true,
            can_go_forward: false,
        });
        app.world_mut().flush();
        assert_eq!(
            sent(&commands),
            vec![WebviewCommand::UrlChanged {
                mount: MountId::new(3),
                url: "https://example.com/next".into(),
            }]
        );
    }

    /// Asserts that a `PageReply` settles the call on the page of its mount,
    /// with a value or with an error.
    ///
    /// Case: a program answers one call and its disconnect rejects another.
    #[test]
    fn a_page_reply_settles_the_call_on_the_page_of_its_mount() {
        let (mut app, _commands) = app();
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        host_event(
            &mut app,
            WebviewEvent::PageReply {
                mount: MountId::new(3),
                page_req: "p0".into(),
                outcome: Ok(json!("done")),
            },
        );
        host_event(
            &mut app,
            WebviewEvent::PageReply {
                mount: MountId::new(3),
                page_req: "p1".into(),
                outcome: Err("owner_disconnected".into()),
            },
        );
        assert_eq!(
            app.world().resource::<Emitted>().0,
            vec![
                (
                    webview,
                    "orzma".to_string(),
                    json!({"reqId": "p0", "ok": true, "value": "done"}),
                ),
                (
                    webview,
                    "orzma".to_string(),
                    json!({"reqId": "p1", "ok": false, "error": "owner_disconnected"}),
                ),
            ]
        );
    }

    /// Asserts that a `PageEvent` reaches the page of its mount on the
    /// `orzma.event` channel.
    ///
    /// Case: a program tells its page to reload the document it shows.
    #[test]
    fn a_program_event_reaches_the_page_of_its_mount() {
        let (mut app, _commands) = app();
        let webview = spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        host_event(
            &mut app,
            WebviewEvent::PageEvent {
                mount: MountId::new(3),
                event: "reload".into(),
                payload: json!({"x": 1}),
            },
        );
        assert_eq!(
            app.world().resource::<Emitted>().0,
            vec![(
                webview,
                "orzma.event".to_string(),
                json!({"event": "reload", "payload": {"x": 1}}),
            )]
        );
    }

    /// Asserts that a reply for a mount with no live webview reaches no page.
    ///
    /// Case: the program answers a call after the page that made it was
    /// unmounted.
    #[test]
    fn a_page_message_for_a_mount_without_a_webview_is_dropped() {
        let (mut app, _commands) = app();
        spawn_mounted(&mut app, MountId::new(3), "orzma://H/index.html");
        host_event(
            &mut app,
            WebviewEvent::PageReply {
                mount: MountId::new(99),
                page_req: "p0".into(),
                outcome: Ok(Value::Null),
            },
        );
        assert!(app.world().resource::<Emitted>().0.is_empty());
    }

    #[derive(Resource, Default)]
    struct SourceChanged(bool);

    fn probe_source_changed(mut probe: ResMut<SourceChanged>, sources: Query<Ref<WebviewSource>>) {
        probe.0 = sources.iter().any(|source| source.is_changed());
    }

    /// Asserts that navigating to the URL already loaded leaves the source
    /// untouched, while a new URL replaces it.
    ///
    /// Case: a TUI browser re-sends its current address, then follows a
    /// link the user picked.
    #[test]
    fn a_navigation_replaces_the_source_only_when_the_url_changes() {
        let (mut app, _commands) = app();
        app.init_resource::<SourceChanged>()
            .add_systems(Update, probe_source_changed);
        let webview = spawn_mounted(&mut app, MountId::new(3), "https://example.com/");
        app.update();
        app.update();
        host_event(
            &mut app,
            WebviewEvent::Navigate {
                mount: MountId::new(3),
                navigation: Navigation::To("https://example.com/".into()),
            },
        );
        app.update();
        assert!(!app.world().resource::<SourceChanged>().0);
        host_event(
            &mut app,
            WebviewEvent::Navigate {
                mount: MountId::new(3),
                navigation: Navigation::To("https://example.com/next".into()),
            },
        );
        app.update();
        assert!(app.world().resource::<SourceChanged>().0);
        assert!(matches!(
            app.world().get::<WebviewSource>(webview),
            Some(WebviewSource::Url(url)) if url == "https://example.com/next"
        ));
    }

    /// Asserts that a navigation to the URL the webview was loaded with,
    /// after the page moved elsewhere on its own, asks CEF to load it once
    /// without touching the source, and that the same navigation does
    /// nothing while that load is in flight or after the page is back there.
    ///
    /// Case: a TUI browser's user follows a link inside the page, then types
    /// the start page's address into the address bar and presses Enter
    /// twice, and sends it once more after the page is back there.
    #[test]
    fn a_navigation_to_the_loaded_url_after_the_page_moved_loads_it() {
        #[derive(Resource, Default)]
        struct Navigated(Vec<(Entity, String)>);
        let (mut app, _commands) = app();
        app.init_resource::<SourceChanged>()
            .init_resource::<Navigated>()
            .add_systems(Update, probe_source_changed)
            .add_observer(
                |ev: On<RequestNavigate>, mut navigated: ResMut<Navigated>| {
                    navigated.0.push((ev.webview, ev.url.clone()));
                },
            );
        let webview = spawn_mounted(&mut app, MountId::new(3), "https://example.com/");
        app.update();
        app.update();
        let arrive = |app: &mut App, url: &str| {
            app.world_mut().trigger(AddressChanged {
                webview,
                url: url.into(),
                can_go_back: true,
                can_go_forward: false,
            });
            app.world_mut().flush();
        };
        let back_home = WebviewEvent::Navigate {
            mount: MountId::new(3),
            navigation: Navigation::To("https://example.com/".into()),
        };
        arrive(&mut app, "https://example.com/story");
        host_event(&mut app, back_home.clone());
        host_event(&mut app, back_home.clone());
        app.update();
        assert!(!app.world().resource::<SourceChanged>().0);
        assert_eq!(
            app.world().resource::<Navigated>().0,
            vec![(webview, "https://example.com/".to_string())]
        );
        arrive(&mut app, "https://example.com/");
        host_event(&mut app, back_home);
        app.update();
        assert_eq!(app.world().resource::<Navigated>().0.len(), 1);
    }

    /// Asserts that a back navigation asks CEF to go back in the webview of
    /// its mount.
    ///
    /// Case: a TUI browser's user presses its back key.
    #[test]
    fn a_back_navigation_asks_cef_to_go_back() {
        #[derive(Resource, Default)]
        struct WentBack(Vec<Entity>);
        let (mut app, _commands) = app();
        app.init_resource::<WentBack>().add_observer(
            |ev: On<RequestGoBack>, mut went: ResMut<WentBack>| {
                went.0.push(ev.webview);
            },
        );
        let webview = spawn_mounted(&mut app, MountId::new(3), "https://example.com/");
        host_event(
            &mut app,
            WebviewEvent::Navigate {
                mount: MountId::new(3),
                navigation: Navigation::Back,
            },
        );
        assert_eq!(app.world().resource::<WentBack>().0, vec![webview]);
    }
}
