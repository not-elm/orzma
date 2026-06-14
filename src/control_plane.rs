//! Native ozmux control plane: a local Unix-socket listener that accepts
//! authenticated dynamic webview registrations (Tier 1) from local programs,
//! mints opaque handles into the `DynamicRegistry`, and tears them down on
//! disconnect or surface despawn. Uses a Tokio-free reader/writer thread model.

use crate::control_plane::listener::{ControlEvent, spawn_listener};
use crate::control_plane::protocol::{RegisterKind, ServerMsg};
use crate::inline_webview::InlineWebview;
use bevy::prelude::*;
use bevy_cef::prelude::HostEmitEvent;
use crossbeam_channel::{Receiver, Sender};
use data_encoding::BASE32_NOPAD;
use ozmux_webview_host::DynAssetRegistry;
use ozmux_webview_host::host::RuntimeRoot;
use ozmux_multiplexer::SurfaceMarker;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

mod listener;
mod protocol;

/// Where a dynamic view's content lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DynSource {
    /// Files served under this absolute root via `ozmux-dyn://`.
    Dir(PathBuf),
    /// A single inline HTML document, registered into `DynAssetRegistry` and
    /// served under `ozmux-dyn://<handle>/`.
    Inline(String),
}

/// A Tier 1 dynamic registration: its content source, entry, input policy, and
/// the terminal surface + control-plane connection that own it (for scoped
/// mount-gating and teardown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DynamicView {
    /// The content source.
    pub(crate) source: DynSource,
    /// HTML entry path relative to a `Dir` root (ignored for `Inline`).
    pub(crate) entry: String,
    /// Whether the mounted webview accepts pointer/keyboard input.
    pub(crate) interactive: bool,
    /// The terminal surface a `mount-inline;<handle>` must originate from. The
    /// registering program's PTY env token resolved to this surface, so only
    /// that surface may mount the handle (tighter than the spec's pane wording).
    pub(crate) owner_surface: Entity,
    /// The control-plane connection that registered it.
    pub(crate) connection_id: u64,
}

/// Stamped on a Tier 1 inline webview entity at mount: the control-plane
/// connection that registered it (back-channel routing target) and its handle.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub(crate) struct WebviewOwner {
    /// The owning connection (push `call` frames here).
    pub(crate) connection_id: u64,
    /// The registration handle (for `emit` fan-out + ownership checks).
    pub(crate) handle: String,
}

/// Maps an opaque `handle` to its dynamic registration. The single Bevy-side
/// registry for Tier 1 (the CEF scheme handler reads the thin `DynAssetRegistry`
/// separately). Carries scoped removal for teardown.
#[derive(Resource, Default)]
pub(crate) struct DynamicRegistry {
    by_handle: HashMap<String, DynamicView>,
}

impl DynamicRegistry {
    /// Resolves a `handle` to its registration, if any.
    pub(crate) fn get(&self, handle: &str) -> Option<&DynamicView> {
        self.by_handle.get(handle)
    }

    /// Inserts/replaces a registration.
    pub(crate) fn insert(&mut self, handle: String, view: DynamicView) {
        self.by_handle.insert(handle, view);
    }

    /// Removes one `handle`, returning its `owner_surface` when it existed.
    pub(crate) fn remove(&mut self, handle: &str) -> Option<Entity> {
        self.by_handle.remove(handle).map(|v| v.owner_surface)
    }

    /// Removes every handle owned by `connection_id`, returning the removed
    /// handles (so the caller can purge the `DynAssetRegistry` too).
    pub(crate) fn remove_by_connection(&mut self, connection_id: u64) -> Vec<String> {
        self.drain_where(|v| v.connection_id == connection_id)
    }

    /// Removes every handle owned by `owner_surface`, returning the removed
    /// handles (so the caller can purge the `DynAssetRegistry` too).
    pub(crate) fn remove_by_surface(&mut self, owner_surface: Entity) -> Vec<String> {
        self.drain_where(|v| v.owner_surface == owner_surface)
    }

    fn drain_where(&mut self, pred: impl Fn(&DynamicView) -> bool) -> Vec<String> {
        let drained: Vec<String> = self
            .by_handle
            .iter()
            .filter(|(_, v)| pred(v))
            .map(|(h, _)| h.clone())
            .collect();
        for h in &drained {
            self.by_handle.remove(h);
        }
        drained
    }
}

/// In-flight `globalReqId → (webview, pageReqId, connection_id)` correlation for
/// the back-channel, plus the Rust-minted id counter. Routes each `window.ozmux`
/// call to the control-plane connection that registered the webview.
#[derive(Resource, Default)]
pub(crate) struct OzmuxRpc {
    inflight: HashMap<String, (Entity, String, u64)>,
    next_id: u64,
}

impl OzmuxRpc {
    /// Mints the next global reqId.
    pub(crate) fn mint(&mut self) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        id
    }

    /// Records an in-flight call.
    pub(crate) fn note(
        &mut self,
        global_id: &str,
        webview: Entity,
        page_req: &str,
        connection_id: u64,
    ) {
        self.inflight.insert(
            global_id.to_string(),
            (webview, page_req.to_string(), connection_id),
        );
    }

    /// Removes and returns the in-flight call for `global_id`, but ONLY when it
    /// was registered by `connection_id`. A mismatching connection leaves the
    /// entry intact and returns `None`.
    ///
    /// # Invariants
    /// The match-before-remove order is load-bearing: global reqIds are a
    /// monotonic counter shared across all connections, so they are guessable. A
    /// foreign program replaying another connection's reqId must NOT be able to
    /// consume (and thereby drop) that connection's pending call — checking
    /// ownership only AFTER removing would orphan the page Promise.
    pub(crate) fn take_for_connection(
        &mut self,
        global_id: &str,
        connection_id: u64,
    ) -> Option<(Entity, String)> {
        match self.inflight.get(global_id) {
            Some((_, _, conn)) if *conn == connection_id => self
                .inflight
                .remove(global_id)
                .map(|(webview, page_req, _)| (webview, page_req)),
            _ => None,
        }
    }

    /// Removes every in-flight call for `connection_id`, returning each
    /// `(webview, pageReqId)` so the caller can reject the page Promise.
    pub(crate) fn drain_connection(&mut self, connection_id: u64) -> Vec<(Entity, String)> {
        self.inflight
            .extract_if(|_, (_, _, c)| *c == connection_id)
            .map(|(_, (e, p, _))| (e, p))
            .collect()
    }

    /// Removes every in-flight call targeting `webview` (despawn prune).
    pub(crate) fn drain_webview(&mut self, webview: Entity) {
        self.inflight.retain(|_, (e, _, _)| *e != webview);
    }

    #[cfg(test)]
    pub(crate) fn count_in_flight_for_test(&self) -> usize {
        self.inflight.len()
    }
}

/// A shared `token → surface` map: the env-injected `$OZMUX_TOKEN` of each PTY
/// resolves to the surface that owns it. Read by the listener thread on `hello`,
/// written when a terminal surface is spawned. `Entity` is stored directly; it
/// is only meaningful inside the same `World` generation.
#[derive(Resource, Clone, Default)]
pub(crate) struct TokenRegistry(Arc<RwLock<HashMap<String, Entity>>>);

impl TokenRegistry {
    /// Resolves a token to the surface that owns it.
    pub(crate) fn resolve(&self, token: &str) -> Option<Entity> {
        self.0.read().unwrap().get(token).copied()
    }

    /// Binds a token to a surface.
    pub(crate) fn insert(&self, token: impl Into<String>, surface: Entity) {
        self.0.write().unwrap().insert(token.into(), surface);
    }
}

/// A shared `connection_id → outbound-line sender` table. Each live control
/// connection owns a writer thread draining a `Sender<String>`; ECS pushes
/// server-initiated `{op:"call",…}` lines here to reach a specific program.
#[derive(Resource, Clone, Default)]
pub(crate) struct ConnectionWriters(Arc<RwLock<HashMap<u64, Sender<String>>>>);

impl ConnectionWriters {
    /// Registers a writer channel for `connection_id`.
    pub(crate) fn insert(&self, connection_id: u64, tx: Sender<String>) {
        self.0.write().unwrap().insert(connection_id, tx);
    }

    /// Removes the writer channel for `connection_id` when the connection closes.
    pub(crate) fn remove(&self, connection_id: u64) {
        self.0.write().unwrap().remove(&connection_id);
    }

    /// Queues one NDJSON line to `connection_id`; returns false if the connection
    /// is gone or its writer has exited.
    pub(crate) fn send(&self, connection_id: u64, line: String) -> bool {
        let guard = self.0.read().unwrap();
        guard
            .get(&connection_id)
            .map(|tx| tx.send(line).is_ok())
            .unwrap_or(false)
    }
}

/// Mints a per-surface env token (same generator as handles).
pub(crate) fn mint_token() -> String {
    mint_id()
}

/// Mints an opaque 128-bit identifier (CSPRNG), base32-encoded (unpadded) and
/// lowercased. The alphabet `a-z2-7` is a subset of the OSC `view_id` charset
/// `^[A-Za-z0-9._-]{1,128}$`, so a minted handle is a valid `mount-inline;<id>`.
///
/// # Invariants
/// The output MUST be lowercase. A handle is used as the host of the
/// `ozmux-dyn://<handle>/` URL, and Chromium canonicalizes (lowercases) the host
/// of a STANDARD-scheme URL before it reaches the scheme handler; an uppercase
/// handle would then miss the case-sensitive `DynAssetRegistry` lookup → 404.
fn mint_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG is available");
    BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
}

/// The receiver of `ControlEvent`s from the listener threads.
#[derive(Resource)]
pub(crate) struct ControlEvents(pub(crate) Receiver<ControlEvent>);

/// The CEF-facing `DynAssetRegistry`, held as a resource so the apply system and
/// teardown can populate/purge `Dir` handles the scheme handler reads.
#[derive(Resource, Clone)]
pub(crate) struct DynAssetRegistryRes(pub(crate) DynAssetRegistry);

/// The bound control-socket path + token registry, surfaced so terminal-surface
/// setup can mint per-surface tokens and inject `$OZMUX_SOCK` / `$OZMUX_TOKEN`.
#[derive(Resource, Clone)]
pub(crate) struct ControlPlaneHandle {
    /// The bound listener socket path (`$OZMUX_SOCK`).
    pub(crate) sock_path: PathBuf,
    /// The shared `token → surface` registry.
    pub(crate) tokens: TokenRegistry,
}

/// Wires the control-plane listener, the event-apply system, and the teardown
/// observer. Takes the `DynAssetRegistry` shared with the `ozmux-dyn` scheme handler.
pub(crate) struct OzmuxControlPlanePlugin {
    dyn_assets: DynAssetRegistry,
}

impl OzmuxControlPlanePlugin {
    /// Builds the plugin sharing `dyn_assets` with the `ozmux-dyn` scheme handler.
    pub(crate) fn new(dyn_assets: DynAssetRegistry) -> Self {
        Self { dyn_assets }
    }
}

impl Plugin for OzmuxControlPlanePlugin {
    fn build(&self, app: &mut App) {
        let tokens = TokenRegistry::default();
        let writers = ConnectionWriters::default();
        match RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), "control") {
            Ok(runtime) => {
                let sock_path = runtime.sock_dir().join("control.sock");
                match spawn_listener(&sock_path, tokens.clone(), writers.clone()) {
                    Ok(events) => {
                        app.insert_resource(ControlEvents(events));
                        app.insert_resource(ControlPlaneHandle { sock_path, tokens });
                        app.insert_resource(ControlRuntime(runtime));
                    }
                    Err(e) => tracing::error!(error = %e, "control-plane listener failed to bind"),
                }
            }
            Err(e) => tracing::error!(error = %e, "control-plane runtime dir failed"),
        }
        app.insert_resource(writers);
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(DynAssetRegistryRes(self.dyn_assets.clone()));
        app.add_systems(Update, apply_control_events);
        app.add_observer(purge_dynamic_on_surface_removed);
    }
}

/// Keeps the control runtime dir alive for the app's lifetime (drop removes the
/// 0700 dir + socket).
#[derive(Resource)]
struct ControlRuntime(
    #[expect(
        dead_code,
        reason = "held only for its Drop impl that removes the 0700 runtime dir"
    )]
    RuntimeRoot,
);

/// Drains queued `ControlEvent`s: mints handles for `register` and populates the
/// `DynamicRegistry` (+ `DynAssetRegistry` for `Dir`), releases on `unregister`,
/// and purges a connection's handles on `Disconnect`.
fn apply_control_events(
    mut commands: Commands,
    mut registry: ResMut<DynamicRegistry>,
    mut rpc: ResMut<OzmuxRpc>,
    events: Option<Res<ControlEvents>>,
    dyn_assets: Res<DynAssetRegistryRes>,
    inline: Query<(Entity, &InlineWebview)>,
) {
    let Some(events) = events else {
        return;
    };
    while let Ok(event) = events.0.try_recv() {
        match event {
            ControlEvent::Register {
                connection_id,
                owner_surface,
                kind,
                reply,
            } => {
                let view = match build_view(kind, owner_surface, connection_id) {
                    Ok(v) => v,
                    Err(code) => {
                        let _ = reply.send(ServerMsg::err(code));
                        continue;
                    }
                };
                let handle = mint_id();
                match &view.source {
                    DynSource::Dir(root) => dyn_assets.0.insert_dir(handle.clone(), root.clone()),
                    DynSource::Inline(html) => {
                        dyn_assets
                            .0
                            .insert_inline(handle.clone(), html.clone().into_bytes());
                    }
                }
                registry.insert(handle.clone(), view);
                let _ = reply.send(ServerMsg::ok(handle));
            }
            ControlEvent::Unregister {
                connection_id,
                handle,
            } => {
                let removed = if registry
                    .get(&handle)
                    .is_some_and(|v| v.connection_id == connection_id)
                {
                    registry.remove(&handle);
                    dyn_assets.0.remove(&handle);
                    vec![handle]
                } else {
                    vec![]
                };
                despawn_mounted(&mut commands, &inline, &removed);
            }
            ControlEvent::Disconnect { connection_id } => {
                let removed = registry.remove_by_connection(connection_id);
                for h in &removed {
                    dyn_assets.0.remove(h);
                }
                despawn_mounted(&mut commands, &inline, &removed);
                for (webview, page_req) in rpc.drain_connection(connection_id) {
                    let payload = serde_json::json!({ "reqId": page_req, "ok": false, "error": "owner_disconnected" });
                    commands.trigger(HostEmitEvent::new(webview, "ozmux", &payload));
                }
            }
            ControlEvent::Reply {
                req_id,
                ok,
                value,
                error,
                connection_id,
            } => {
                // NOTE: take_for_connection drops a reply whose sending connection
                // is not the one that originated the call, WITHOUT consuming the
                // pending entry — a foreign program replaying another connection's
                // (monotonic, guessable) global reqId must not settle or drop its call.
                let Some((webview, page_req)) = rpc.take_for_connection(&req_id, connection_id)
                else {
                    continue;
                };
                let payload = if ok {
                    serde_json::json!({ "reqId": page_req, "ok": true, "value": value })
                } else {
                    serde_json::json!({ "reqId": page_req, "ok": false, "error": error.unwrap_or_default() })
                };
                commands.trigger(HostEmitEvent::new(webview, "ozmux", &payload));
            }
            ControlEvent::Emit {
                connection_id,
                handle,
                event,
                payload,
            } => {
                let owns = registry
                    .get(&handle)
                    .is_some_and(|v| v.connection_id == connection_id);
                if !owns {
                    continue;
                }
                let frame = serde_json::json!({ "event": event, "payload": payload });
                for (entity, view) in &inline {
                    if view.view_id == handle {
                        commands.trigger(HostEmitEvent::new(entity, "ozmux.event", &frame));
                    }
                }
            }
        }
    }
}

/// Despawn observer: when a terminal surface goes away (tab close, or pane
/// despawn cascading to its surfaces), purge every dynamic registration owned by
/// that surface. Mirrors the multiplexer's existing `on_remove_*` observers.
fn purge_dynamic_on_surface_removed(
    ev: On<Remove, SurfaceMarker>,
    mut registry: ResMut<DynamicRegistry>,
    dyn_assets: Res<DynAssetRegistryRes>,
) {
    for handle in registry.remove_by_surface(ev.entity) {
        dyn_assets.0.remove(&handle);
    }
}

fn despawn_mounted(
    commands: &mut Commands,
    inline: &Query<(Entity, &InlineWebview)>,
    removed: &[String],
) {
    for (entity, view) in inline {
        if removed.contains(&view.view_id) {
            commands.entity(entity).despawn();
        }
    }
}

/// Validates a `RegisterKind` and builds a `DynamicView`. Returns a short error
/// code for an unsafe entry, a missing/relative root, or oversized inline HTML.
fn build_view(
    kind: RegisterKind,
    owner_surface: Entity,
    connection_id: u64,
) -> Result<DynamicView, &'static str> {
    match kind {
        RegisterKind::Dir {
            root,
            entry,
            interactive,
        } => {
            let root_path = PathBuf::from(&root);
            if !root_path.is_absolute() || !root_path.is_dir() {
                return Err("invalid_root");
            }
            if !is_safe_entry(&entry) {
                return Err("unsafe_entry");
            }
            Ok(DynamicView {
                source: DynSource::Dir(root_path),
                entry,
                interactive,
                owner_surface,
                connection_id,
            })
        }
        RegisterKind::Inline { html, interactive } => {
            if html.len() > MAX_INLINE_HTML {
                return Err("html_too_large");
            }
            Ok(DynamicView {
                source: DynSource::Inline(html),
                entry: "index.html".into(),
                interactive,
                owner_surface,
                connection_id,
            })
        }
    }
}

/// True when `entry` is a non-empty relative path of normal components only
/// (no `..`, `.`, or leading `/`). Same shape as `asset::is_safe_rel_path`.
fn is_safe_entry(entry: &str) -> bool {
    let p = std::path::Path::new(entry);
    !p.as_os_str().is_empty()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Upper bound on a single inline HTML document (4 MiB).
const MAX_INLINE_HTML: usize = 4 * 1024 * 1024;

#[cfg(test)]
mod token_tests {
    use super::*;
    use bevy::prelude::Entity;

    #[test]
    fn minted_ids_match_the_osc_view_id_charset() {
        for _ in 0..50 {
            let id = mint_id();
            assert!(
                !id.is_empty() && id.len() <= 128,
                "id length out of range: {id:?}"
            );
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'),
                "minted id {id} must satisfy the OSC charset"
            );
            assert!(
                !id.chars().any(|c| c.is_ascii_uppercase()),
                "minted id {id} must be lowercase — it is used as an ozmux-dyn:// host that Chromium lowercases"
            );
        }
    }

    #[test]
    fn minted_ids_are_unique() {
        let a = mint_id();
        let b = mint_id();
        assert_ne!(a, b);
    }

    #[test]
    fn token_registry_binds_and_resolves() {
        let reg = TokenRegistry::default();
        reg.insert("tok", Entity::from_bits(5));
        assert_eq!(reg.resolve("tok"), Some(Entity::from_bits(5)));
        assert_eq!(reg.resolve("nope"), None);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use bevy::prelude::Entity;

    fn surface(n: u32) -> Entity {
        Entity::from_bits(n as u64)
    }

    #[test]
    fn insert_then_get_roundtrips() {
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "h1".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: surface(1),
                connection_id: 7,
            },
        );
        assert_eq!(reg.get("h1").map(|v| v.interactive), Some(true));
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn remove_by_connection_drops_only_that_connections_handles() {
        let mut reg = DynamicRegistry::default();
        reg.insert("a".into(), view(surface(1), 7));
        reg.insert("b".into(), view(surface(1), 7));
        reg.insert("c".into(), view(surface(2), 8));

        let removed = reg.remove_by_connection(7);
        assert_eq!(removed.len(), 2);
        assert!(reg.get("a").is_none() && reg.get("b").is_none());
        assert!(reg.get("c").is_some());
    }

    #[test]
    fn remove_by_surface_drops_only_that_surfaces_handles() {
        let mut reg = DynamicRegistry::default();
        reg.insert("a".into(), view(surface(1), 7));
        reg.insert("c".into(), view(surface(2), 8));

        let removed = reg.remove_by_surface(surface(1));
        assert_eq!(removed, vec!["a".to_string()]);
        assert!(reg.get("a").is_none());
        assert!(reg.get("c").is_some());
    }

    #[test]
    fn remove_one_returns_the_owner_surface_when_present() {
        let mut reg = DynamicRegistry::default();
        reg.insert("a".into(), view(surface(3), 9));
        assert_eq!(reg.remove("a"), Some(surface(3)));
        assert_eq!(reg.remove("a"), None);
    }

    fn view(owner: Entity, conn: u64) -> DynamicView {
        DynamicView {
            source: DynSource::Dir("/abs".into()),
            entry: "index.html".into(),
            interactive: true,
            owner_surface: owner,
            connection_id: conn,
        }
    }
}

#[cfg(test)]
mod apply_tests {
    use super::*;
    use crossbeam_channel::{bounded, unbounded};

    #[test]
    fn apply_register_dir_mints_handle_and_populates_both_registries() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let dyn_assets = DynAssetRegistry::default();
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(dyn_assets.clone()));
        app.add_systems(Update, apply_control_events);

        let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
        ev_tx
            .send(ControlEvent::Register {
                connection_id: 1,
                owner_surface: Entity::from_bits(11),
                kind: RegisterKind::Dir {
                    root: dir.path().to_string_lossy().into_owned(),
                    entry: "index.html".into(),
                    interactive: true,
                },
                reply: reply_tx,
            })
            .unwrap();

        app.update();

        let handle = match reply_rx.try_recv().expect("apply must reply") {
            ServerMsg::Ok { handle, .. } => handle,
            ServerMsg::Err { error, .. } => panic!("unexpected err: {error}"),
        };
        assert!(
            dyn_assets.get(&handle).is_some(),
            "DynAssetRegistry populated for Dir"
        );
        assert!(
            app.world()
                .resource::<DynamicRegistry>()
                .get(&handle)
                .is_some(),
            "DynamicRegistry populated"
        );
    }

    #[test]
    fn apply_register_inline_populates_dyn_asset_registry_with_html_bytes() {
        use ozmux_webview_host::DynAsset;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let dyn_assets = DynAssetRegistry::default();
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(dyn_assets.clone()));
        app.add_systems(Update, apply_control_events);

        let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
        ev_tx
            .send(ControlEvent::Register {
                connection_id: 1,
                owner_surface: Entity::from_bits(11),
                kind: RegisterKind::Inline {
                    html: "<h1>x</h1>".into(),
                    interactive: true,
                },
                reply: reply_tx,
            })
            .unwrap();

        app.update();

        let handle = match reply_rx.try_recv().expect("apply must reply") {
            ServerMsg::Ok { handle, .. } => handle,
            ServerMsg::Err { error, .. } => panic!("unexpected err: {error}"),
        };
        assert!(
            matches!(dyn_assets.get(&handle), Some(DynAsset::Inline(bytes)) if bytes == b"<h1>x</h1>"),
            "DynAssetRegistry must carry the inline HTML bytes for the minted handle"
        );
    }

    #[test]
    fn apply_register_invalid_root_replies_err() {
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(DynAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);

        let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
        ev_tx
            .send(ControlEvent::Register {
                connection_id: 1,
                owner_surface: Entity::from_bits(1),
                kind: RegisterKind::Dir {
                    root: "/nonexistent/abs/xyz".into(),
                    entry: "index.html".into(),
                    interactive: true,
                },
                reply: reply_tx,
            })
            .unwrap();
        app.update();
        assert!(
            matches!(reply_rx.try_recv(), Ok(ServerMsg::Err { .. })),
            "invalid root must reply Err"
        );
    }

    #[test]
    fn apply_disconnect_purges_that_connections_handles() {
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let dyn_assets = DynAssetRegistry::default();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "h".into(),
            DynamicView {
                source: DynSource::Dir("/x".into()),
                entry: "i".into(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 5,
            },
        );
        dyn_assets.insert_dir("h", "/x".into());
        app.insert_resource(reg);
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(dyn_assets.clone()));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Disconnect { connection_id: 5 })
            .unwrap();
        app.update();
        assert!(app.world().resource::<DynamicRegistry>().get("h").is_none());
        assert!(dyn_assets.get("h").is_none());
    }

    #[test]
    fn apply_is_a_noop_when_control_events_missing() {
        let mut app = App::new();
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(DynAssetRegistryRes(DynAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);
        app.update();
        assert!(app.world().get_resource::<ControlEvents>().is_none());
    }

    #[test]
    fn surface_despawn_purges_its_dynamic_registrations() {
        let mut app = App::new();
        app.add_observer(purge_dynamic_on_surface_removed);
        let dyn_assets = DynAssetRegistry::default();
        let surface = app.world_mut().spawn(SurfaceMarker).id();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "h".into(),
            DynamicView {
                source: DynSource::Dir("/x".into()),
                entry: "i".into(),
                interactive: true,
                owner_surface: surface,
                connection_id: 1,
            },
        );
        dyn_assets.insert_dir("h", "/x".into());
        app.insert_resource(reg);
        app.insert_resource(DynAssetRegistryRes(dyn_assets.clone()));

        app.world_mut().entity_mut(surface).despawn();

        assert!(
            app.world().resource::<DynamicRegistry>().get("h").is_none(),
            "purged from DynamicRegistry on surface despawn"
        );
        assert!(
            dyn_assets.get("h").is_none(),
            "purged from DynAssetRegistry"
        );
    }

    #[test]
    fn disconnect_despawns_mounted_webviews_for_its_handles() {
        use crate::inline_webview::InlineWebview;
        let mut app = App::new();
        let dyn_assets = DynAssetRegistry::default();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "HMOUNT".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 9,
            },
        );
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mounted = app
            .world_mut()
            .spawn(InlineWebview {
                view_id: "HMOUNT".into(),
                instance_id: None,
                slot: 0,
            })
            .id();
        app.insert_resource(reg);
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(DynAssetRegistryRes(dyn_assets));
        app.insert_resource(ControlEvents(ev_rx));
        app.add_systems(Update, apply_control_events);
        ev_tx
            .send(ControlEvent::Disconnect { connection_id: 9 })
            .unwrap();
        app.update();
        assert!(
            app.world().get_entity(mounted).is_err(),
            "mounted webview for a disconnected handle must be despawned"
        );
        assert!(
            app.world()
                .resource::<DynamicRegistry>()
                .get("HMOUNT")
                .is_none()
        );
    }

    #[test]
    fn apply_reply_reemits_to_the_originating_webview() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut rpc = OzmuxRpc::default();
        let webview = app.world_mut().spawn_empty().id();
        let g = rpc.mint();
        rpc.note(&g, webview, "p9", 5);
        app.insert_resource(rpc);
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(DynAssetRegistry::default()));
        #[derive(Resource, Default)]
        struct Caps(Vec<(Entity, String, serde_json::Value)>);
        app.insert_resource(Caps::default());
        app.add_observer(|e: On<HostEmitEvent>, mut c: ResMut<Caps>| {
            let payload: serde_json::Value = serde_json::from_str(&e.payload).unwrap_or_default();
            c.0.push((e.webview, e.id.clone(), payload));
        });
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Reply {
                req_id: g.clone(),
                ok: true,
                value: serde_json::json!(99),
                error: None,
                connection_id: 5,
            })
            .unwrap();
        app.update();

        let caps = app.world().resource::<Caps>();
        assert_eq!(caps.0.len(), 1);
        let (wv, channel, payload) = &caps.0[0];
        assert_eq!(*wv, webview);
        assert_eq!(channel, "ozmux");
        assert_eq!(payload["reqId"], "p9");
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["value"], 99);
    }

    #[test]
    fn apply_reply_from_wrong_connection_does_not_drop_the_pending_call() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut rpc = OzmuxRpc::default();
        let webview = app.world_mut().spawn_empty().id();
        let g = rpc.mint();
        rpc.note(&g, webview, "p9", 5);
        app.insert_resource(rpc);
        app.insert_resource(DynamicRegistry::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(DynAssetRegistry::default()));
        #[derive(Resource, Default)]
        struct Caps(Vec<(Entity, String)>);
        app.insert_resource(Caps::default());
        app.add_observer(|e: On<HostEmitEvent>, mut c: ResMut<Caps>| {
            c.0.push((e.webview, e.id.clone()));
        });
        app.add_systems(Update, apply_control_events);

        // A reply replaying the (monotonic, guessable) global reqId from a DIFFERENT
        // connection must be dropped WITHOUT consuming the pending entry...
        ev_tx
            .send(ControlEvent::Reply {
                req_id: g.clone(),
                ok: true,
                value: serde_json::json!(1),
                error: None,
                connection_id: 9,
            })
            .unwrap();
        // ...so the legitimate reply from the originating connection still settles.
        ev_tx
            .send(ControlEvent::Reply {
                req_id: g.clone(),
                ok: true,
                value: serde_json::json!(2),
                error: None,
                connection_id: 5,
            })
            .unwrap();
        app.update();

        assert_eq!(
            app.world().resource::<Caps>().0,
            vec![(webview, "ozmux".to_string())],
            "only the owning connection's reply settles the page"
        );
    }

    #[test]
    fn apply_emit_broadcasts_only_to_owning_connections_handle() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "H".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 5,
            },
        );
        let mounted = app
            .world_mut()
            .spawn((
                InlineWebview {
                    view_id: "H".into(),
                    instance_id: None,
                    slot: 0,
                },
                WebviewOwner {
                    connection_id: 5,
                    handle: "H".into(),
                },
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(DynAssetRegistryRes(DynAssetRegistry::default()));
        #[derive(Resource, Default)]
        struct Caps(Vec<(Entity, String)>);
        app.insert_resource(Caps::default());
        app.add_observer(|e: On<HostEmitEvent>, mut c: ResMut<Caps>| {
            c.0.push((e.webview, e.id.clone()))
        });
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Emit {
                connection_id: 5,
                handle: "H".into(),
                event: "tick".into(),
                payload: serde_json::json!(1),
            })
            .unwrap();
        ev_tx
            .send(ControlEvent::Emit {
                connection_id: 99,
                handle: "H".into(),
                event: "tick".into(),
                payload: serde_json::json!(1),
            })
            .unwrap();
        app.update();

        let caps = app.world().resource::<Caps>();
        assert_eq!(caps.0, vec![(mounted, "ozmux.event".to_string())]);
    }
}

#[cfg(test)]
mod back_channel_state_tests {
    use super::*;
    use bevy::prelude::Entity;

    #[test]
    fn ozmux_rpc_take_for_connection_matches_only_the_owning_connection() {
        let mut rpc = OzmuxRpc::default();
        let g = rpc.mint();
        assert_eq!(g, "0");
        rpc.note(&g, Entity::from_bits(2), "p1", 5);
        // A mismatching connection must NOT consume the entry.
        assert!(rpc.take_for_connection(&g, 999).is_none());
        let taken = rpc.take_for_connection(&g, 5).expect("present");
        assert_eq!(taken, (Entity::from_bits(2), "p1".to_string()));
        assert!(rpc.take_for_connection(&g, 5).is_none());
    }

    #[test]
    fn ozmux_rpc_drains_a_connections_inflight() {
        let mut rpc = OzmuxRpc::default();
        let a = rpc.mint();
        let b = rpc.mint();
        rpc.note(&a, Entity::from_bits(1), "p", 5);
        rpc.note(&b, Entity::from_bits(2), "p", 9);
        let drained = rpc.drain_connection(5);
        assert_eq!(drained, vec![(Entity::from_bits(1), "p".to_string())]);
        assert!(rpc.take_for_connection(&a, 5).is_none());
        assert!(rpc.take_for_connection(&b, 9).is_some());
    }

    #[test]
    fn ozmux_rpc_drain_webview_drops_only_that_webviews_calls() {
        let mut rpc = OzmuxRpc::default();
        let a = rpc.mint();
        let b = rpc.mint();
        rpc.note(&a, Entity::from_bits(1), "p", 5);
        rpc.note(&b, Entity::from_bits(2), "p", 5);
        rpc.drain_webview(Entity::from_bits(1));
        assert!(rpc.take_for_connection(&a, 5).is_none());
        assert!(rpc.take_for_connection(&b, 5).is_some());
    }
}
