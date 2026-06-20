//! CEF webview wiring for the `window.ozma` Tier 1 back-channel: registers the
//! `ozma-dyn://` dynamic asset scheme, keeps `bevy_cef`'s `FocusedWebview` in
//! step with the active pane, and routes the `ozma.call` frames the page bridge
//! emits to the registering program over the control socket.

use crate::control_plane::{ConnectionWriters, OzmuxRpc, WebviewOwner};
use crate::inline_webview::InlineWebview;
use crate::osc_webview::NonInteractive;
use crate::system_set::OzmuxSystems;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_tmux::{ActivePane, TmuxPane};
use ozmux_webview_host::DynAssetRegistry;
use ozmux_webview_host::dyn_scheme::custom_dyn_scheme;
use serde_json::Value;
use std::path::PathBuf;

pub(crate) mod preload;

/// One frame emitted by the page bridge (`ozma_bridge.js`) via
/// `cef.emit({ kind: '…', … })`, inspected by the per-kind observers.
///
/// `#[serde(transparent)]` makes it deserialize from the bare emitted object
/// (`{kind, reqId, …}`), not from a `{"0": …}` wrapper — `bevy_cef`'s
/// `cef.emit(frame)` serializes only its first argument into one global
/// `Receive<OzmuxFrame>`.
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
struct OzmuxFrame(serde_json::Value);

/// The `kind` discriminator routing a `Receive<OzmuxFrame>` to the Tier 1
/// back-channel (`on_ozmux_call_frame`). The page side emits it in `ozma_bridge.js`.
const OZMA_CALL_KIND: &str = "ozma.call";

/// Builds the `CefPlugin` with the `ozma-dyn://` (dynamic, Tier 1) scheme bound
/// to its shared `DynAssetRegistry`, using `root_cache_path` as this process's
/// unique CEF profile directory (one Chromium singleton lock per instance).
pub(crate) fn cef_plugin(dyn_registry: DynAssetRegistry, root_cache_path: PathBuf) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_dyn_scheme(dyn_registry)],
        command_line_config: cef_command_line_config(),
        root_cache_path: Some(root_cache_path.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

/// CEF command-line switches for the embedded webview. The `debug` feature adds
/// `remote-debugging-port` — a local Chromium DevTools (CDP) endpoint on
/// `127.0.0.1:9222` for inspecting the embedded webview — and is off by default
/// so that endpoint is never exposed in normal builds. `CommandLineConfig::default()`
/// already carries the macOS `use-mock-keychain` switch in either case.
fn cef_command_line_config() -> CommandLineConfig {
    let config = CommandLineConfig::default();
    #[cfg(feature = "debug")]
    let config = config.with_switch_value("remote-debugging-port", "9222");
    config
}

/// Wires the `window.ozma` Tier 1 back-channel: the `ozma.call` frame
/// observer, the webview-load loggers, and the focus sync that keeps
/// `bevy_cef`'s `FocusedWebview` in step with the active pane.
pub struct OzmuxWebviewRenderPlugin;

impl Plugin for OzmuxWebviewRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(JsEmitEventPlugin::<OzmuxFrame>::default())
            .add_observer(on_ozmux_call_frame)
            .add_observer(drop_ozmux_inflight_on_webview_despawn)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error)
            .add_systems(Update, sync_focused_webview.after(OzmuxSystems::Input));
    }
}

/// Keeps `bevy_cef`'s `FocusedWebview` in step with ozmux's active pane.
///
/// bevy_cef only updates `FocusedWebview` when a *webview* node is clicked
/// (`set_focus_on_press`), so moving focus to a terminal pane (a non-webview)
/// leaves the webview focused: its DOM text area keeps the caret and
/// `send_key_event` keeps routing keystrokes to it. Driving `FocusedWebview`
/// from the active pane fixes both — keyboard follows the focused pane, and CEF
/// blurs the webview on focus-leave (`bevy_cef`'s `apply_webview_focus` releases
/// CEF focus when `FocusedWebview` becomes `None`).
///
/// One case is PRESERVED instead of driven: when `FocusedWebview` holds an
/// inline webview child (`InlineWebview`) whose `ChildOf` parent is a live
/// `TmuxPane` — active or not — that inline focus stands (spec §7, single
/// focus source). This covers click-granted focus and the app-declared focus
/// set via the control-plane `SetFocus` op, and means switching the active
/// pane does NOT clear an inline webview's focus: the webview keeps keyboard
/// focus until its child despawns (or focus moves off it), at which point the
/// sync falls through to the clear path below, which maps the active terminal
/// pane to `None`.
pub(crate) fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    active_pane: Query<Entity, (With<TmuxPane>, With<ActivePane>)>,
    webviews: Query<(), With<WebviewSource>>,
    non_interactive: Query<(), With<NonInteractive>>,
    inline_parents: Query<&ChildOf, With<InlineWebview>>,
    tmux_panes: Query<(), With<TmuxPane>>,
) {
    // NOTE: a despawned inline child fails `inline_parents.get` here and so
    // falls through to the clear path below, which resolves to `None` and
    // clears it — that fall-through is the GC for tmux-pane inline focus; a
    // later edit that short-circuits this arm before the despawn check would
    // leak focus.
    if let Some(child) = focused.0
        && let Ok(parent) = inline_parents.get(child)
        && tmux_panes.contains(parent.parent())
    {
        return;
    }

    let active_surface = active_pane.iter().next();
    let active = active_surface
        .filter(|surface| webviews.contains(*surface) && !non_interactive.contains(*surface));
    if focused.0 != active {
        focused.0 = active;
    }
}

/// Inbound (Tier 1 back-channel): a `window.ozma.call` arrives as a
/// `Receive<OzmuxFrame>` with `kind:"ozma.call"`. The trusted caller is
/// `frame.webview` (bound per-webview by `bevy_cef`, never the JS payload); its
/// `WebviewOwner` names the registering connection. The call is forwarded over
/// that connection's writer under a Rust-minted global reqId; a missing
/// owner/connection rejects the page Promise directly.
///
/// Registered as an observer on the shared `Receive<OzmuxFrame>` event (NOT a
/// second `JsEmitEventPlugin`): the event carries all frames; non-`ozma.call`
/// frames are ignored via the early return on `OZMA_CALL_KIND`.
fn on_ozmux_call_frame(
    frame: On<Receive<OzmuxFrame>>,
    mut commands: Commands,
    mut rpc: ResMut<OzmuxRpc>,
    writers: Res<ConnectionWriters>,
    owners: Query<&WebviewOwner>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(OZMA_CALL_KIND) {
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
        reject_ozmux_call(&mut commands, webview, req_id, "no_owner");
        return;
    };
    let global_id = rpc.mint();
    let line = serde_json::json!({
        "op": "call", "handle": owner.handle, "reqId": global_id, "method": method, "params": params
    })
    .to_string();
    if !writers.send(owner.connection_id, line) {
        reject_ozmux_call(&mut commands, webview, req_id, "owner_unavailable");
        return;
    }
    rpc.note(&global_id, webview, req_id, owner.connection_id);
}

/// Emits a `{reqId, ok:false, error}` reply to one webview on the `"ozma"`
/// channel (settling the page Promise).
fn reject_ozmux_call(commands: &mut Commands, webview: Entity, req_id: &str, error: &str) {
    let payload = serde_json::json!({ "reqId": req_id, "ok": false, "error": error });
    commands.trigger(HostEmitEvent::new(webview, "ozma", &payload));
}

/// Despawn prune: drop a despawned webview's in-flight back-channel calls.
fn drop_ozmux_inflight_on_webview_despawn(
    remove: On<Remove, WebviewOwner>,
    mut rpc: ResMut<OzmuxRpc>,
) {
    rpc.drain_webview(remove.entity);
}

/// Logs the start of a webview page load. Debug-level diagnostics: these
/// observers fire for every `bevy_cef` webview, not only ozmux webviews.
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
    fn focused_webview_follows_active_pane() {
        // Regression: moving focus to a terminal pane must clear FocusedWebview,
        // so bevy_cef blurs the webview (releasing its DOM text area
        // and stopping keyboard from routing to it). When the webview pane is
        // active, its webview must be focused.
        use ozmux_tmux::{ActivePane, PaneId, TmuxPane};
        use tmux_control_parser::CellDims;

        let dims = CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        // The active TmuxPane IS the active surface. The webview pane carries a
        // WebviewSource; the terminal pane does not.
        let terminal_pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims,
            })
            .id();
        let ext_pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims,
                },
                WebviewSource::new("ozma-dyn://memo/index.html"),
            ))
            .id();

        let set_active = move |app: &mut App, active: Entity, inactive: Entity| {
            app.world_mut().entity_mut(active).insert(ActivePane);
            app.world_mut().entity_mut(inactive).remove::<ActivePane>();
            app.update();
        };

        set_active(&mut app, ext_pane, terminal_pane);
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(ext_pane),
            "active webview pane must focus its webview"
        );

        set_active(&mut app, terminal_pane, ext_pane);
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
        use ozmux_tmux::{ActivePane, PaneId, TmuxPane};
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        // The active TmuxPane carries a NonInteractive WebviewSource: it must
        // never be focused.
        app.world_mut().spawn((
            TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
            },
            ActivePane,
            WebviewSource::new("ozma-dyn://memo/index.html"),
            NonInteractive,
        ));

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "NonInteractive webview surface must never become FocusedWebview"
        );
    }

    #[test]
    fn tmux_pane_inline_focus_is_preserved() {
        use ozmux_tmux::{PaneId, TmuxPane};
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                InlineWebview {
                    view_id: "v".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "an inline child of a live TmuxPane must keep FocusedWebview across the per-frame sync",
        );
    }

    #[test]
    fn tmux_pane_inline_focus_is_gc_on_despawn() {
        use ozmux_tmux::{PaneId, TmuxPane};
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                InlineWebview {
                    view_id: "v".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        app.world_mut().entity_mut(child).despawn();

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "a despawned inline child must be GC'd out of FocusedWebview",
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
                "kind": "ozma.call", "reqId": "p0", "method": "save", "params": [1, 2]
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
                .resource::<OzmuxRpc>()
                .count_in_flight_for_test(),
            1
        );
    }
}
