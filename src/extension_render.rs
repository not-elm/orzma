//! CEF host-RPC plumbing (currently dormant) plus the `window.ozmux` Tier 1
//! back-channel: registers the `ozmux-dyn://` dynamic asset scheme, keeps
//! `bevy_cef`'s `FocusedWebview` in step with the active pane, and routes the
//! `host.call` / `ozmux.call` frames the page bridges emit. The `window.<ns>`
//! host-API path (`on_host_call_frame` → single Node host via `HostRpc`) is kept
//! intact but dormant: nothing grants namespaces yet, so every call is denied
//! until per-webview API registration is wired.

use crate::control_plane::{ConnectionWriters, OzmuxRpc, WebviewOwner};
use crate::inline_webview::{InlineWebview, focused_inline_of};
use crate::osc_webview::GrantedNamespaces;
use crate::osc_webview::NonInteractive;
use crate::system_set::OzmuxSystems;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::DynAssetRegistry;
use ozmux_extension_host::HostRpcClient;
use ozmux_extension_host::dyn_scheme::custom_dyn_scheme;
use ozmux_multiplexer::{AttachedWorkspace, MultiplexerCommands, SurfaceMarker, WorkspaceMarker};
use serde_json::Value;

pub(crate) mod preload;

/// One frame emitted by the page bridge (`host_bridge.js` or `ozmux_bridge.js`)
/// via `cef.emit({ kind: '…', … })`, inspected by the per-kind observers.
///
/// `#[serde(transparent)]` makes it deserialize from the bare emitted object
/// (`{kind, reqId, …}`), not from a `{"0": …}` wrapper — `bevy_cef`'s
/// `cef.emit(frame)` serializes only its first argument into one global
/// `Receive<OzmuxFrame>`.
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
struct OzmuxFrame(serde_json::Value);

/// The connected host RPC client plus the in-flight `globalReqId → (webview,
/// pageReqId)` correlation. `globalReqId` is minted Rust-side (a monotonic
/// counter) so page-local `reqId`s — which collide across webviews — are never
/// used as a routing key. Populated by `extension_manager` on host readiness.
#[derive(Resource, Default)]
pub(crate) struct HostRpc {
    client: Option<HostRpcClient>,
    inflight: HashMap<String, (Entity, String)>,
    next_id: u64,
}

impl HostRpc {
    /// Installs a freshly-connected client, clearing any stale correlation /
    /// counter from a previous host generation.
    pub(crate) fn set_client(&mut self, client: HostRpcClient) {
        self.client = Some(client);
        self.inflight.clear();
        self.next_id = 0;
    }

    /// Drops the client and clears in-flight correlation (host exited):
    /// subsequent calls reject `host_unavailable`. `next_id` is reset by the
    /// following `set_client`, not here. In-flight calls awaiting a host reply
    /// are dropped without settling their page Promise (Phase 1 has no per-call
    /// timeout); the page sees a hung Promise until reload — acceptable under the
    /// no-auto-restart scope.
    pub(crate) fn clear_client(&mut self) {
        self.client = None;
        self.inflight.clear();
    }

    #[cfg(test)]
    pub(crate) fn note_in_flight_for_test(
        &mut self,
        global_id: &str,
        webview: Entity,
        local: &str,
    ) {
        self.inflight
            .insert(global_id.to_string(), (webview, local.to_string()));
    }

    #[cfg(test)]
    pub(crate) fn count_in_flight_for_test(&self) -> usize {
        self.inflight.len()
    }
}

/// The `kind` discriminator that routes a `Receive<OzmuxFrame>` to the new-model
/// host-API path (`on_host_call_frame`). The page side emits the matching literal
/// in `host_bridge.js`.
const HOST_CALL_KIND: &str = "host.call";

/// The `kind` discriminator routing a `Receive<OzmuxFrame>` to the Tier 1
/// back-channel (`on_ozmux_call_frame`). The page side emits it in `ozmux_bridge.js`.
const OZMUX_CALL_KIND: &str = "ozmux.call";

/// Builds the `CefPlugin` with the `ozmux-dyn://` (dynamic, Tier 1) scheme bound
/// to its shared `DynAssetRegistry`.
pub fn cef_plugin(dyn_registry: DynAssetRegistry) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_dyn_scheme(dyn_registry)],
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

/// Wires the host-RPC plumbing (dormant) and the `window.ozmux` Tier 1
/// back-channel: the `host.call` / `ozmux.call` frame observers, the host-reply
/// drain, the webview-load loggers, and the focus sync that keeps
/// `bevy_cef`'s `FocusedWebview` in step with the active pane.
pub struct OzmuxExtensionRenderPlugin;

impl Plugin for OzmuxExtensionRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JsEmitEventPlugin::<OzmuxFrame>::default())
            .init_resource::<HostRpc>()
            .add_observer(on_host_call_frame)
            .add_observer(drop_inflight_host_calls_on_webview_despawn)
            .add_observer(on_ozmux_call_frame)
            .add_observer(drop_ozmux_inflight_on_webview_despawn)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error)
            .add_systems(
                Update,
                (
                    drain_host_rpc_responses,
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
/// One case is PRESERVED instead of driven: when `FocusedWebview` holds an
/// inline webview child (`InlineWebview`) whose `ChildOf` parent is the
/// resolved active surface, the click-granted inline focus stands (spec §7,
/// single focus source). Without this arm the per-frame sync would map the
/// active terminal surface to `None` and clobber an inline click one frame
/// after `dispatch_mouse_buttons` set it. Every other case — a different pane
/// or surface becoming active, the inline child despawning, a tab-type
/// webview surface — keeps the drive-from-active-pane behavior above.
fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    webviews: Query<(), With<WebviewSource>>,
    non_interactive: Query<(), With<NonInteractive>>,
    inline_parents: Query<&ChildOf, With<InlineWebview>>,
) {
    let active_surface = attached_workspace
        .iter()
        .next()
        .and_then(|workspace| mux.workspaces_active_pane(workspace))
        .and_then(|pane| mux.panes_active_surface(pane));
    if focused_inline_of(Some(&focused), &inline_parents, active_surface).is_some() {
        return;
    }
    let active = active_surface
        .filter(|surface| webviews.contains(*surface) && !non_interactive.contains(*surface));
    if focused.0 != active {
        focused.0 = active;
    }
}

/// Inbound (new-model host API): a `window.<ns>.<method>` call arrives as a
/// `Receive<OzmuxFrame>` with `kind:"host.call"`. The trusted caller is
/// `frame.webview` (bound per-webview by `bevy_cef`, never the JS payload); its
/// `GrantedNamespaces` decides whether the call may proceed. Allowed calls are
/// forwarded to the single host over a Rust-minted global `reqId`; denied or
/// host-down calls reject the page-local Promise directly.
///
/// Registered as an observer on the shared `Receive<OzmuxFrame>` event (NOT a
/// second `JsEmitEventPlugin`): the event carries all frames; non-`host.call`
/// frames are ignored via the early return on `HOST_CALL_KIND`.
fn on_host_call_frame(
    frame: On<Receive<OzmuxFrame>>,
    mut commands: Commands,
    mut host_rpc: ResMut<HostRpc>,
    granted: Query<&GrantedNamespaces>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(HOST_CALL_KIND) {
        return;
    }
    let webview = frame.webview;
    let req_id = payload
        .get("reqId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let ns = payload
        .get("ns")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let allowed = granted
        .get(webview)
        .map(|g| g.0.contains(ns))
        .unwrap_or(false);
    if !allowed {
        reject_host_call(
            &mut commands,
            webview,
            req_id,
            &format!("capability_denied: {ns}"),
        );
        return;
    }
    if host_rpc.client.is_none() {
        reject_host_call(&mut commands, webview, req_id, "host_unavailable");
        return;
    }

    let global_id = host_rpc.next_id.to_string();
    host_rpc.next_id += 1;
    let args = payload
        .get("args")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let line = serde_json::json!({
        "reqId": global_id, "ns": ns, "method": method, "args": args
    })
    .to_string();
    host_rpc
        .client
        .as_ref()
        .expect("client present: guarded by the is_none() check above")
        .send_line(line);
    host_rpc
        .inflight
        .insert(global_id, (webview, req_id.to_string()));
}

/// Emits a `{reqId, ok:false, error}` reply to a single webview on the `"ozmux"`
/// channel, settling the page-local Promise.
fn reject_host_call(commands: &mut Commands, webview: Entity, req_id: &str, error: &str) {
    let payload = serde_json::json!({ "reqId": req_id, "ok": false, "error": error });
    commands.trigger(HostEmitEvent::new(webview, "ozmux", &payload));
}

/// Inbound (Tier 1 back-channel): a `window.ozmux.call` arrives as a
/// `Receive<OzmuxFrame>` with `kind:"ozmux.call"`. The trusted caller is
/// `frame.webview` (bound per-webview by `bevy_cef`, never the JS payload); its
/// `WebviewOwner` names the registering connection. The call is forwarded over
/// that connection's writer under a Rust-minted global reqId; a missing
/// owner/connection rejects the page Promise directly.
///
/// Registered as an observer on the shared `Receive<OzmuxFrame>` event (NOT a
/// second `JsEmitEventPlugin`): the event carries all frames; non-`ozmux.call`
/// frames are ignored via the early return on `OZMUX_CALL_KIND`.
fn on_ozmux_call_frame(
    frame: On<Receive<OzmuxFrame>>,
    mut commands: Commands,
    mut rpc: ResMut<OzmuxRpc>,
    writers: Res<ConnectionWriters>,
    owners: Query<&WebviewOwner>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(OZMUX_CALL_KIND) {
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
    let args = payload
        .get("args")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    let Ok(owner) = owners.get(webview) else {
        reject_ozmux_call(&mut commands, webview, req_id, "no_owner");
        return;
    };
    let global_id = rpc.mint();
    let line = serde_json::json!({
        "op": "call", "handle": owner.handle, "reqId": global_id, "method": method, "args": args
    })
    .to_string();
    if !writers.send(owner.connection_id, line) {
        reject_ozmux_call(&mut commands, webview, req_id, "owner_unavailable");
        return;
    }
    rpc.note(&global_id, webview, req_id, owner.connection_id);
}

/// Emits a `{reqId, ok:false, error}` reply to one webview on the `"ozmux"`
/// channel (settling the page Promise).
fn reject_ozmux_call(commands: &mut Commands, webview: Entity, req_id: &str, error: &str) {
    let payload = serde_json::json!({ "reqId": req_id, "ok": false, "error": error });
    commands.trigger(HostEmitEvent::new(webview, "ozmux", &payload));
}

/// Despawn prune: drop a despawned webview's in-flight back-channel calls.
fn drop_ozmux_inflight_on_webview_despawn(
    remove: On<Remove, WebviewOwner>,
    mut rpc: ResMut<OzmuxRpc>,
) {
    rpc.drain_webview(remove.entity);
}

/// Outbound (new-model host API): drains the host's NDJSON reply lines, maps the
/// Rust-minted global `reqId` back to its `(webview, pageReqId)`, restores the
/// page-local `reqId`, and re-emits each reply to the originating webview on the
/// `"ozmux"` channel. A reply with no live in-flight entry (surface despawned)
/// is dropped.
fn drain_host_rpc_responses(mut commands: Commands, mut host_rpc: ResMut<HostRpc>) {
    let mut lines = Vec::new();
    if let Some(client) = host_rpc.client.as_ref() {
        while let Some(line) = client.try_recv_response() {
            lines.push(line);
        }
    }
    for line in lines {
        let Ok(mut frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(global_id) = frame
            .get("reqId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let Some((webview, local_id)) = host_rpc.inflight.remove(&global_id) else {
            continue;
        };
        frame["reqId"] = Value::String(local_id);
        commands.trigger(HostEmitEvent::new(webview, "ozmux", &frame));
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

/// Drops any in-flight host RPC calls originating from a surface/webview that is
/// being despawned, so their replies are not routed back to a dead entity.
fn drop_inflight_host_calls_on_webview_despawn(
    ev: On<Remove, SurfaceMarker>,
    mut host_rpc: ResMut<HostRpc>,
) {
    host_rpc
        .inflight
        .retain(|_, (entity, _)| *entity != ev.entity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

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
            .insert(WebviewSource::new("ozmux-dyn://memo/index.html"));

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
    fn non_interactive_webview_surface_never_takes_keyboard_focus() {
        use crate::osc_webview::NonInteractive;
        use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin, Side, SplitOrientation};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let (workspace, _pane) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane)
            })
            .unwrap();
        app.world_mut().flush();

        let render_only_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(_pane, Side::After, SplitOrientation::Horizontal)
                    .expect("split_pane")
            })
            .unwrap();
        app.world_mut().flush();

        let render_only_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_active_surface(render_only_pane)
                    .expect("render-only surface")
            })
            .unwrap();
        app.world_mut()
            .entity_mut(workspace)
            .insert(AttachedWorkspace);
        app.world_mut().entity_mut(render_only_surface).insert((
            WebviewSource::new("ozmux-dyn://memo/index.html"),
            NonInteractive,
        ));

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(workspace, render_only_pane)
                    .expect("set_active_pane");
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "NonInteractive webview surface must never become FocusedWebview"
        );
    }

    #[test]
    fn sync_preserves_inline_focus_on_the_active_surface_and_clears_on_pane_switch() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin, Side, SplitOrientation};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let (workspace, terminal_pane, terminal_surface) = app
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

        let child = app
            .world_mut()
            .spawn((
                ChildOf(terminal_surface),
                InlineWebview {
                    view_id: "inline-test".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();
        app.insert_resource(FocusedWebview(Some(child)));

        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "an inline-focused child of the ACTIVE terminal surface must survive the sync"
        );

        // Splitting promotes the fresh pane to active; the focused child's
        // parent is no longer the active surface, so the preservation arm
        // must NOT hold and the terminal-pane mapping (None) must win.
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(terminal_pane, Side::After, SplitOrientation::Horizontal)
                    .expect("split_pane")
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "inline focus must clear once a different pane/surface becomes active"
        );
    }

    #[test]
    fn surface_despawn_drops_its_in_flight_host_calls() {
        use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<HostRpc>();
        app.add_observer(drop_inflight_host_calls_on_webview_despawn);

        let (pane, surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut()
            .resource_mut::<HostRpc>()
            .note_in_flight_for_test("0", surface, "h0");

        assert_eq!(
            app.world().resource::<HostRpc>().count_in_flight_for_test(),
            1,
            "the in-flight call must be tracked before despawn"
        );

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.close_surface(pane, surface).expect("close_surface");
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert_eq!(
            app.world().resource::<HostRpc>().count_in_flight_for_test(),
            0,
            "the surface's in-flight host RPC must be dropped on despawn"
        );
    }

    use std::collections::HashSet;

    #[derive(Resource, Default)]
    struct CapturedEmits(Vec<(Entity, String)>);

    fn capture_emits(ev: On<HostEmitEvent>, mut cap: ResMut<CapturedEmits>) {
        cap.0.push((ev.webview, ev.payload.clone()));
    }

    fn gate_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HostRpc>();
        app.init_resource::<CapturedEmits>();
        app.add_observer(on_host_call_frame);
        app.add_observer(capture_emits);
        app
    }

    fn host_call(req_id: &str, ns: &str, method: &str) -> OzmuxFrame {
        OzmuxFrame(serde_json::json!({
            "kind": "host.call", "reqId": req_id, "ns": ns, "method": method, "args": []
        }))
    }

    #[test]
    fn host_call_denied_for_ungranted_namespace_is_not_forwarded() {
        let mut app = gate_app();
        let mut caps = HashSet::new();
        caps.insert("clipboard".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });
        app.world_mut().flush();

        assert!(
            app.world().resource::<HostRpc>().inflight.is_empty(),
            "a denied call must NOT be forwarded (no in-flight entry)"
        );
        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0.len(), 1, "exactly one reject emitted");
        assert_eq!(cap.0[0].0, webview);
        assert!(
            cap.0[0].1.contains("capability_denied"),
            "rejected as capability_denied"
        );
        assert!(
            cap.0[0].1.contains("\"reqId\":\"h0\""),
            "reply carries the page-local reqId"
        );
    }

    #[test]
    fn host_call_trust_key_is_the_webview_entity_not_the_payload() {
        let mut app = gate_app();
        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let _granted = app.world_mut().spawn(GrantedNamespaces(caps)).id();
        let caller = app
            .world_mut()
            .spawn(GrantedNamespaces(HashSet::new()))
            .id();

        app.world_mut().trigger(Receive {
            webview: caller,
            payload: OzmuxFrame(serde_json::json!({
                "kind": "host.call", "reqId": "h0", "ns": "fs", "method": "read",
                "args": [], "surfaceId": "spoofed", "granted": ["fs"]
            })),
        });
        app.world_mut().flush();

        assert!(app.world().resource::<HostRpc>().inflight.is_empty());
        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0.len(), 1);
        assert!(cap.0[0].1.contains("capability_denied"));
    }

    #[test]
    fn host_call_rejects_when_host_unavailable() {
        let mut app = gate_app();
        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });
        app.world_mut().flush();

        assert!(app.world().resource::<HostRpc>().inflight.is_empty());
        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0.len(), 1);
        assert!(cap.0[0].1.contains("host_unavailable"));
    }

    #[test]
    fn host_call_for_granted_namespace_is_forwarded_and_tracked() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut r = BufReader::new(stream);
                let mut line = String::new();
                while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    line.clear();
                }
            }
        });

        let mut app = gate_app();
        let client = ozmux_extension_host::HostRpcClient::connect(&sock).unwrap();
        app.world_mut().resource_mut::<HostRpc>().set_client(client);

        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });

        let hr = app.world().resource::<HostRpc>();
        assert_eq!(hr.inflight.len(), 1, "an allowed call is tracked in-flight");
        let entry = hr.inflight.values().next().unwrap();
        assert_eq!(entry.0, webview);
        assert_eq!(
            entry.1.as_str(),
            "h0",
            "in-flight maps the global id back to the page-local reqId"
        );
        app.world_mut().flush();
        assert!(
            app.world().resource::<CapturedEmits>().0.is_empty(),
            "an allowed call is forwarded, not rejected"
        );

        app.world_mut().resource_mut::<HostRpc>().clear_client();
        let _ = server.join();
    }

    #[test]
    fn host_reply_routed_back_to_origin_with_page_local_req_id() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
            let gid = frame
                .get("reqId")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            let mut w = stream;
            w.write_all(
                format!("{{\"reqId\":\"{gid}\",\"ok\":true,\"value\":\"hi\"}}\n").as_bytes(),
            )
            .unwrap();
            w.flush().unwrap();
        });

        let mut app = gate_app();
        app.add_systems(Update, drain_host_rpc_responses);
        let client = ozmux_extension_host::HostRpcClient::connect(&sock).unwrap();
        app.world_mut().resource_mut::<HostRpc>().set_client(client);

        let mut caps = std::collections::HashSet::new();
        caps.insert("fs".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            app.update();
            if !app.world().resource::<CapturedEmits>().0.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reply never routed back"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0[0].0, webview, "reply targets the originating webview");
        assert!(
            cap.0[0].1.contains("\"reqId\":\"h0\""),
            "page-local reqId restored"
        );
        assert!(
            cap.0[0].1.contains("\"value\":\"hi\""),
            "value forwarded through"
        );
        assert!(
            app.world().resource::<HostRpc>().inflight.is_empty(),
            "the in-flight entry is consumed on reply"
        );

        app.world_mut().resource_mut::<HostRpc>().clear_client();
        let _ = server.join();
    }

    #[test]
    fn pruning_drops_in_flight_calls_for_a_despawned_surface() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HostRpc>();
        app.add_observer(drop_inflight_host_calls_on_webview_despawn);

        let surface = app.world_mut().spawn(SurfaceMarker).id();
        let other = app.world_mut().spawn(SurfaceMarker).id();
        {
            let mut hr = app.world_mut().resource_mut::<HostRpc>();
            hr.note_in_flight_for_test("0", surface, "h0");
            hr.note_in_flight_for_test("1", other, "h1");
        }

        app.world_mut().entity_mut(surface).despawn();

        assert_eq!(
            app.world().resource::<HostRpc>().count_in_flight_for_test(),
            1,
            "prune must drop ONLY the despawned surface's in-flight calls (retain, not clear)"
        );
    }

    #[test]
    fn ozmux_call_frame_pushes_call_to_owner_connection() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(OzmuxRpc::default());
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_ozmux_call_frame);

        let webview = app
            .world_mut()
            .spawn(WebviewOwner {
                connection_id: 7,
                handle: "H".into(),
            })
            .id();

        app.world_mut().trigger(Receive {
            webview,
            payload: OzmuxFrame(serde_json::json!({
                "kind": "ozmux.call", "reqId": "p0", "method": "save", "args": [1, 2]
            })),
        });

        let line = rx.try_recv().expect("a call was pushed");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "call");
        assert_eq!(v["handle"], "H");
        assert_eq!(v["method"], "save");
        assert_eq!(v["reqId"], "0");
        assert_eq!(
            app.world()
                .resource::<OzmuxRpc>()
                .count_in_flight_for_test(),
            1
        );
    }
}
