//! Native orzma control plane: a local Unix-socket listener that accepts
//! authenticated dynamic webview registrations (Tier 1) from local programs,
//! mints opaque handles into the `OrzmaRegistry`, and tears them down on
//! disconnect or surface despawn. Uses a Tokio-free reader/writer thread model.

use crate::control_plane::listener::{ControlEvent, spawn_listener};
use crate::control_plane::protocol::{HostKeyChord, NavAction, RegisterKind, ServerMsg};
use crate::webview::apc::NonInteractive;
use crate::webview::mount::Webview;
use bevy::prelude::*;
use bevy_cef::prelude::FocusedWebview;
use bevy_cef::prelude::HostEmitEvent;
use bevy_cef::prelude::{RequestGoBack, RequestGoForward, RequestReload, WebviewSource};
use bevy_orzma_tty::prelude::{OrzmaTtyHandle, RequestTtyWebviewRemove};
use crossbeam_channel::{Receiver, Sender};
use data_encoding::BASE32_NOPAD;
use orzma_vt::prelude::InstanceId;
use orzma_webview_host::WebviewAssetRegistry;
use orzma_webview_host::host::RuntimeRoot;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use url::Url;

mod listener;
mod protocol;

pub(crate) use protocol::PushMsg;

/// A forward-key chord normalized to host input types: a bevy `KeyCode` plus
/// modifier booleans. Used to suppress CEF double-delivery and to match keys
/// for PTY forwarding (design spec §E type normalization).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedChord {
    /// The base key as a bevy `KeyCode`.
    pub code: KeyCode,
    /// Alt modifier active.
    pub alt: bool,
    /// Ctrl modifier active.
    pub ctrl: bool,
    /// Shift modifier active.
    pub shift: bool,
    /// The Super/Command/Meta modifier (bevy calls it Super/logo).
    pub logo: bool,
}

/// Where a dynamic view's content lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrzmaSource {
    /// Files served under this absolute root via `orzma://`.
    Dir(PathBuf),
    /// A single inline HTML document, registered into `WebviewAssetRegistry` and
    /// served under `orzma://<handle>/`.
    Inline(String),
    /// A remote `http(s)` URL loaded directly by CEF (no `orzma://` origin,
    /// no `WebviewAssetRegistry` entry). `bridge` records whether the registering
    /// program opted into the `window.orzma` back-channel.
    Url {
        /// The validated `http(s)` URL.
        url: String,
        /// Whether the `window.orzma` back-channel is injected.
        bridge: bool,
    },
}

impl OrzmaSource {
    /// Whether a mounted view of this source receives the `window.orzma`
    /// back-channel. Only a display-only (`bridge: false`) `Url` source is
    /// unbridged; `Dir`/`Inline` are always bridged.
    pub(crate) fn is_bridged(&self) -> bool {
        match self {
            OrzmaSource::Dir(_) | OrzmaSource::Inline(_) => true,
            OrzmaSource::Url { bridge, .. } => *bridge,
        }
    }

    /// Whether this source is a remote `http(s)` URL (vs `Dir`/`Inline`).
    pub(crate) fn is_url(&self) -> bool {
        matches!(self, OrzmaSource::Url { .. })
    }
}

/// A Tier 1 dynamic registration: its content source, entry, input policy, and
/// the terminal surface + control-plane connection that own it (for scoped
/// mount-gating and teardown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrzmaView {
    /// The content source.
    pub source: OrzmaSource,
    /// HTML entry path relative to a `Dir` root (ignored for `Inline` and `Url`).
    pub entry: String,
    /// Whether the mounted webview accepts pointer/keyboard input.
    pub interactive: bool,
    /// The terminal surface a `mount;<handle>` must originate from. The
    /// registering program's PTY env token resolved to this surface, so only
    /// that surface may mount the handle (tighter than the spec's pane wording).
    pub owner_surface: Entity,
    /// The control-plane connection that registered it.
    pub connection_id: u64,
    /// The normalized forward-key chords for this view, derived from the
    /// `register` wire payload. Copied onto the mounted webview entity as
    /// `ForwardKeys` so Phase-4 systems can read them off the focused child.
    pub forward_keys: Vec<NormalizedChord>,
    /// User-supplied preload scripts, copied verbatim from the register wire
    /// and injected onto the mounted webview's `PreloadScripts` after the host
    /// bridge/hints. No size cap or validation (the registering program is
    /// local and trusted).
    pub preload: Vec<String>,
    /// Every instance minted for this registration, in mint order. The
    /// registry keeps this list and its `by_instance` reverse index in
    /// lockstep, so releasing the handle releases exactly these ids.
    pub instances: Vec<InstanceId>,
}

/// Stamped on a Tier 1 webview entity at mount: the control-plane
/// connection that registered it (back-channel routing target), its handle,
/// and the instance the mount was addressed by.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub(crate) struct WebviewOwner {
    /// The owning connection (push `call` frames here).
    pub connection_id: u64,
    /// The registration handle (for `emit` fan-out + ownership checks).
    pub handle: HandleId,
    /// The placement this webview was mounted under, so a back-channel
    /// frame names which of a handle's placements it came from.
    pub instance: InstanceId,
}

/// The opaque identity of one dynamic registration.
///
/// It is the host of that registration's `orzma://<handle>/` origin, the
/// routing key for its back-channel, and the ownership unit `unregister`
/// and connection teardown act on.
///
/// # Invariants
///
/// There is deliberately no `From<HandleId> for String`. That absence is
/// what stops a handle from flowing into an `impl Into<String>` parameter
/// that wants an instance id; adding the conversion for convenience
/// silently reopens that path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandleId(String);

impl HandleId {
    /// Borrows the wire spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// NOTE: the inbound direction is safe and needed (the wire hands us
// strings); it is the outbound `From<HandleId> for String` that must not
// exist, or a handle flows into an `impl Into<String>` slot wanting an
// instance id.
impl From<&str> for HandleId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for HandleId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl Borrow<str> for HandleId {
    // NOTE: lets `HashMap<HandleId, _>` be looked up by `&str` without
    // allocating a HandleId for every probe.
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Maps an opaque `handle` to its dynamic registration, and every minted
/// instance back to the handle it belongs to.
///
/// The single Bevy-side registry for Tier 1 (the CEF scheme handler reads
/// the thin `WebviewAssetRegistry` separately). Carries scoped removal for
/// teardown.
///
/// # Invariants
///
/// The two maps are only ever touched through `&mut self` methods on this
/// type, so an instance is in `by_instance` exactly while its handle's
/// `OrzmaView.instances` lists it.
#[derive(Resource, Default)]
pub(crate) struct OrzmaRegistry {
    by_handle: HashMap<HandleId, OrzmaView>,
    by_instance: HashMap<InstanceId, HandleId>,
}

/// One registration the registry released, with everything its teardown
/// needs: the asset key, the terminal to clear, and the placements it
/// reserved.
pub(crate) struct RemovedRegistration {
    /// The released handle, still the `WebviewAssetRegistry` key.
    pub handle: HandleId,
    /// The surface that owned the registration.
    pub owner_surface: Entity,
    /// Every instance the released handle had minted.
    pub instances: Vec<InstanceId>,
}

impl OrzmaRegistry {
    /// Resolves a `handle` to its registration, if any.
    pub fn get(&self, handle: &HandleId) -> Option<&OrzmaView> {
        self.by_handle.get(handle)
    }

    /// Resolves an instance to its handle and registration, if any.
    pub fn resolve_instance(&self, instance: InstanceId) -> Option<(&HandleId, &OrzmaView)> {
        let handle = self.by_instance.get(&instance)?;
        let view = self.by_handle.get(handle)?;
        Some((handle, view))
    }

    /// Inserts a registration with no instances yet.
    ///
    /// The caller mints its first instance with [`Self::mint_instance`]
    /// immediately afterwards, so every id — the first one included —
    /// comes from the one minting path.
    pub fn insert(&mut self, handle: HandleId, view: OrzmaView) {
        self.by_handle.insert(handle, view);
    }

    /// Mints an instance for `handle`; `None` only when the handle is
    /// unknown.
    ///
    /// This is the ONLY path that produces an [`InstanceId`], which is what
    /// makes "every allocated instance is unique across the registry" hold.
    pub fn mint_instance(&mut self, handle: &HandleId) -> Option<InstanceId> {
        if !self.by_handle.contains_key(handle) {
            return None;
        }
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).expect("OS CSPRNG is available");
        let id = InstanceId::from_bytes(bytes);
        debug_assert!(
            !self.by_instance.contains_key(&id),
            "128 bits of CSPRNG collided; the draw is broken"
        );
        self.by_instance.insert(id, handle.clone());
        self.by_handle
            .get_mut(handle)
            .expect("the handle was present above")
            .instances
            .push(id);
        Some(id)
    }

    /// Removes one `handle` and everything its teardown needs.
    pub fn remove(&mut self, handle: &HandleId) -> Option<RemovedRegistration> {
        let view = self.by_handle.remove(handle)?;
        for id in &view.instances {
            self.by_instance.remove(id);
        }
        Some(RemovedRegistration {
            handle: handle.clone(),
            owner_surface: view.owner_surface,
            instances: view.instances,
        })
    }

    /// Removes every handle owned by `connection_id`.
    pub fn remove_by_connection(&mut self, connection_id: u64) -> Vec<RemovedRegistration> {
        self.drain_where(|v| v.connection_id == connection_id)
    }

    /// Removes every handle owned by `owner_surface`.
    pub fn remove_by_surface(&mut self, owner_surface: Entity) -> Vec<RemovedRegistration> {
        self.drain_where(|v| v.owner_surface == owner_surface)
    }

    fn drain_where(&mut self, pred: impl Fn(&OrzmaView) -> bool) -> Vec<RemovedRegistration> {
        let handles: Vec<HandleId> = self
            .by_handle
            .iter()
            .filter(|(_, v)| pred(v))
            .map(|(h, _)| h.clone())
            .collect();
        handles.iter().filter_map(|h| self.remove(h)).collect()
    }
}

/// In-flight `globalReqId → (webview, pageReqId, connection_id)` correlation for
/// the back-channel, plus the Rust-minted id counter. Routes each `window.orzma`
/// call to the control-plane connection that registered the webview.
#[derive(Resource, Default)]
pub(crate) struct OrzmaRpc {
    inflight: HashMap<String, (Entity, String, u64)>,
    next_id: u64,
}

impl OrzmaRpc {
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

/// A shared `token → surface` map: the env-injected `$ORZMA_TOKEN` of each PTY
/// resolves to the surface that owns it. Read by the listener thread on `hello`,
/// written when a terminal surface is spawned. `Entity` is stored directly; it
/// is only meaningful inside the same `World` generation.
#[derive(Resource, Clone, Default)]
pub struct TokenRegistry(Arc<RwLock<HashMap<String, Entity>>>);

impl TokenRegistry {
    /// Resolves a token to the surface that owns it.
    pub fn resolve(&self, token: &str) -> Option<Entity> {
        self.0.read().unwrap().get(token).copied()
    }

    /// Binds a token to a surface.
    pub fn insert(&self, token: impl Into<String>, surface: Entity) {
        self.0.write().unwrap().insert(token.into(), surface);
    }

    /// Drops every binding that resolves to `surface`. Called when a surface
    /// despawns so a recycled `Entity` id cannot resolve a stale key.
    pub fn remove_entity(&self, surface: Entity) {
        self.0.write().unwrap().retain(|_, bound| *bound != surface);
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

/// Mints an opaque 128-bit [`HandleId`] (CSPRNG), base32-encoded (unpadded)
/// and lowercased. The alphabet `a-z2-7` keeps a minted handle spellable in
/// a URL host, which is where it is used.
///
/// # Invariants
/// The output MUST be lowercase. A handle is used as the host of the
/// `orzma://<handle>/` URL, and Chromium canonicalizes (lowercases) the host
/// of a STANDARD-scheme URL before it reaches the scheme handler; an uppercase
/// handle would then miss the case-sensitive `WebviewAssetRegistry` lookup → 404.
fn mint_handle_id() -> HandleId {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG is available");
    HandleId(BASE32_NOPAD.encode(&bytes).to_ascii_lowercase())
}

/// The receiver of `ControlEvent`s from the listener threads.
#[derive(Resource)]
pub(crate) struct ControlEvents(pub(crate) Receiver<ControlEvent>);

/// The CEF-facing `WebviewAssetRegistry`, held as a resource so the apply system and
/// teardown can populate/purge `Dir` handles the scheme handler reads.
#[derive(Resource, Clone)]
pub(crate) struct WebviewAssetRegistryRes(pub(crate) WebviewAssetRegistry);

/// The bound control-socket path + token registry, surfaced so terminal-surface
/// setup can mint per-surface tokens and inject `$ORZMA_SOCK` / `$ORZMA_TOKEN`.
#[derive(Resource, Clone)]
pub struct ControlPlaneHandle {
    /// The bound listener socket path (`$ORZMA_SOCK`).
    pub sock_path: PathBuf,
    /// The shared `token → surface` registry.
    pub tokens: TokenRegistry,
}

impl ControlPlaneHandle {
    /// Returns the `ORZMA_SOCK` / `ORZMA_TOKEN` env pairs to inject into `surface`'s
    /// PTY so a program running in it can reach the control plane and resolve to
    /// `surface`. Pure: derives the token but does NOT register it — call
    /// [`bind_surface`](Self::bind_surface) after the PTY actually spawns, so a
    /// failed spawn leaks no binding.
    pub fn surface_env(&self, surface: Entity) -> [(String, String); 2] {
        [
            (
                "ORZMA_SOCK".to_string(),
                self.sock_path.to_string_lossy().into_owned(),
            ),
            ("ORZMA_TOKEN".to_string(), surface_token(surface)),
        ]
    }

    /// Registers `token -> surface` so a connecting program's `$ORZMA_TOKEN`
    /// resolves to `surface`. Call only after the surface's PTY has spawned.
    pub fn bind_surface(&self, surface: Entity) {
        self.tokens.insert(surface_token(surface), surface);
    }
}

/// Derives the per-surface token (`orzma:<entity-bits>`) from a surface entity.
/// Single-sources the format so [`ControlPlaneHandle::surface_env`] and
/// [`ControlPlaneHandle::bind_surface`] always agree.
fn surface_token(surface: Entity) -> String {
    format!("orzma:{}", surface.to_bits())
}

/// Wires the control-plane listener, the event-apply system, and the teardown
/// observer. Takes the `WebviewAssetRegistry` shared with the `orzma` scheme handler.
pub(crate) struct ControlPlanePlugin {
    orzma_assets: WebviewAssetRegistry,
}

impl ControlPlanePlugin {
    /// Builds the plugin sharing `orzma_assets` with the `orzma` scheme handler.
    pub(crate) fn new(orzma_assets: WebviewAssetRegistry) -> Self {
        Self { orzma_assets }
    }
}

impl Plugin for ControlPlanePlugin {
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
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(WebviewAssetRegistryRes(self.orzma_assets.clone()));
        app.add_systems(Update, (apply_control_events, gc_despawned_surfaces));
    }
}

/// Purges a despawned surface's dynamic registrations + assets. Keyed on
/// `RemovedComponents<OrzmaTtyHandle>` so it fires for every terminal surface
/// with no multiplexer dependency.
///
/// # Invariants
/// Must stay ungated and run every frame: `RemovedComponents` buffers clear at
/// end of frame, so a skipped frame leaks registrations + assets. The purge
/// also runs when `ControlPlaneHandle` is absent (token unbinding is then a
/// no-op) — gating it behind the handle would leak in that case.
fn gc_despawned_surfaces(
    mut registry: ResMut<OrzmaRegistry>,
    mut closed: RemovedComponents<OrzmaTtyHandle>,
    handle: Option<Res<ControlPlaneHandle>>,
    orzma_assets: Res<WebviewAssetRegistryRes>,
) {
    for entity in closed.read() {
        for removed in registry.remove_by_surface(entity) {
            orzma_assets.0.remove(removed.handle.as_str());
        }
        if let Some(handle) = handle.as_ref() {
            handle.tokens.remove_entity(entity);
        }
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
/// `OrzmaRegistry` (+ `WebviewAssetRegistry` for `Dir`), releases on `unregister`,
/// and purges a connection's handles on `Disconnect`.
fn apply_control_events(
    mut commands: Commands,
    mut registry: ResMut<OrzmaRegistry>,
    mut rpc: ResMut<OrzmaRpc>,
    mut focused: Option<ResMut<FocusedWebview>>,
    mut sources: Query<&mut WebviewSource>,
    events: Option<Res<ControlEvents>>,
    orzma_assets: Res<WebviewAssetRegistryRes>,
    webviews: Query<(Entity, &Webview)>,
    child_of: Query<&ChildOf>,
    non_interactive: Query<(), With<NonInteractive>>,
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
                let handle = mint_handle_id();
                match &view.source {
                    OrzmaSource::Dir(root) => {
                        orzma_assets.0.insert_dir(handle.as_str(), root.clone())
                    }
                    OrzmaSource::Inline(html) => {
                        orzma_assets
                            .0
                            .insert_inline(handle.as_str(), html.clone().into_bytes());
                    }
                    OrzmaSource::Url { .. } => {}
                }
                registry.insert(handle.clone(), view);
                // NOTE: a failed mint must not leave a registered handle with
                // no instance — the program could never mount it, and the
                // asset entry would leak until the connection drops.
                let Some(instance) = registry.mint_instance(&handle) else {
                    registry.remove(&handle);
                    orzma_assets.0.remove(handle.as_str());
                    let _ = reply.send(ServerMsg::err("internal"));
                    continue;
                };
                let _ = reply.send(ServerMsg::registered(handle, instance.to_string()));
            }
            ControlEvent::NewInstance {
                connection_id,
                handle,
                reply,
            } => {
                let owned = registry
                    .get(&handle)
                    .is_some_and(|v| v.connection_id == connection_id);
                if !owned {
                    let code = if registry.get(&handle).is_some() {
                        "not_owner"
                    } else {
                        "unknown_handle"
                    };
                    let _ = reply.send(ServerMsg::err(code));
                    continue;
                }
                match registry.mint_instance(&handle) {
                    Some(instance) => {
                        let _ = reply.send(ServerMsg::instanced(instance.to_string()));
                    }
                    None => {
                        let _ = reply.send(ServerMsg::err("internal"));
                    }
                }
            }
            ControlEvent::Unregister {
                connection_id,
                handle,
            } => {
                let removed: Vec<RemovedRegistration> = if registry
                    .get(&handle)
                    .is_some_and(|v| v.connection_id == connection_id)
                {
                    orzma_assets.0.remove(handle.as_str());
                    registry.remove(&handle).into_iter().collect()
                } else {
                    vec![]
                };
                release_registrations(&mut commands, &webviews, &removed);
            }
            ControlEvent::Disconnect { connection_id } => {
                let removed = registry.remove_by_connection(connection_id);
                for entry in &removed {
                    orzma_assets.0.remove(entry.handle.as_str());
                }
                release_registrations(&mut commands, &webviews, &removed);
                for (webview, page_req) in rpc.drain_connection(connection_id) {
                    let payload = serde_json::json!({ "reqId": page_req, "ok": false, "error": "owner_disconnected" });
                    commands.trigger(HostEmitEvent::new(webview, "orzma", &payload));
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
                commands.trigger(HostEmitEvent::new(webview, "orzma", &payload));
            }
            ControlEvent::Emit {
                connection_id,
                handle,
                event,
                payload,
            } => {
                let deliver = registry
                    .get(&handle)
                    .is_some_and(|v| v.connection_id == connection_id && v.source.is_bridged());
                if !deliver {
                    continue;
                }
                let frame = serde_json::json!({ "event": event, "payload": payload });
                for (entity, view) in &webviews {
                    if view.handle == handle {
                        commands.trigger(HostEmitEvent::new(entity, "orzma.event", &frame));
                    }
                }
            }
            ControlEvent::SetFocus {
                connection_id,
                owner_surface,
                instance,
            } => {
                let Some(focused) = focused.as_mut() else {
                    continue;
                };
                let Some(instance) = instance else {
                    let owned_current = focused
                        .0
                        .is_some_and(|e| child_of.get(e).map(|c| c.parent()) == Ok(owner_surface));
                    if owned_current {
                        focused.0 = None;
                    }
                    continue;
                };
                let Ok(id) = instance.parse::<InstanceId>() else {
                    continue;
                };
                let owned = registry
                    .resolve_instance(id)
                    .is_some_and(|(_, v)| v.connection_id == connection_id);
                if !owned {
                    tracing::debug!(%instance, "focus op for an unowned instance, dropping");
                    continue;
                }
                let target = webviews.iter().find(|(entity, view)| {
                    view.instance == id
                        && child_of.get(*entity).map(|c| c.parent()) == Ok(owner_surface)
                        && !non_interactive.contains(*entity)
                });
                match target {
                    Some((entity, _)) => focused.0 = Some(entity),
                    None => tracing::debug!(
                        %instance,
                        "focus op for an unmounted/non-interactive instance, dropping"
                    ),
                }
            }
            ControlEvent::Navigate {
                connection_id,
                owner_surface,
                instance,
                action,
            } => {
                let Ok(id) = instance.parse::<InstanceId>() else {
                    continue;
                };
                let Some((_, view)) = registry.resolve_instance(id) else {
                    continue;
                };
                if view.connection_id != connection_id {
                    tracing::debug!(%instance, "navigate for an unowned instance, dropping");
                    continue;
                }
                let is_url = view.source.is_url();
                let target = webviews.iter().find(|(entity, v)| {
                    v.instance == id
                        && child_of.get(*entity).map(|c| c.parent()) == Ok(owner_surface)
                });
                let Some((entity, _)) = target else {
                    tracing::debug!(%instance, "navigate for an unmounted instance, dropping");
                    continue;
                };
                match action {
                    NavAction::To(url) => {
                        if !is_url {
                            tracing::debug!(%instance, "navigate To on a non-url view, dropping");
                            continue;
                        }
                        match validate_url_source(&url) {
                            Ok(valid) => {
                                if let Ok(mut source) = sources.get_mut(entity) {
                                    // Mutate only on a real change so navigating to the
                                    // URL already loaded does not fire a spurious CEF
                                    // reload (WebviewSource has no PartialEq for set_if_neq).
                                    let unchanged = matches!(
                                        &*source,
                                        WebviewSource::Url(cur) if *cur == valid
                                    );
                                    if !unchanged {
                                        *source = WebviewSource::Url(valid);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(%instance, error = e, "navigate To rejected url");
                            }
                        }
                    }
                    NavAction::Back => commands.trigger(RequestGoBack { webview: entity }),
                    NavAction::Forward => commands.trigger(RequestGoForward { webview: entity }),
                    NavAction::Reload => commands.trigger(RequestReload { webview: entity }),
                }
            }
        }
    }
}

/// Tears down the registrations `removed` released: despawns their mounted
/// webviews and hands the reservations back to the VT on the surface that
/// owned them.
///
/// The VT cannot learn that a registration is gone — that fact lives on the
/// control socket — so without the second step each released placement keeps
/// a per-terminal cap slot until its anchor scrolls out of history.
fn release_registrations(
    commands: &mut Commands,
    webviews: &Query<(Entity, &Webview)>,
    removed: &[RemovedRegistration],
) {
    for (entity, view) in webviews {
        if removed.iter().any(|entry| entry.handle == view.handle) {
            commands.entity(entity).despawn();
        }
    }
    for entry in removed {
        if entry.instances.is_empty() {
            continue;
        }
        commands.trigger(RequestTtyWebviewRemove {
            terminal: entry.owner_surface,
            instances: entry.instances.clone(),
        });
    }
}

/// Validates a `RegisterKind` and builds a `OrzmaView`. Returns a short error
/// code for an unsafe entry, a missing/relative root, or oversized inline HTML.
fn build_view(
    kind: RegisterKind,
    owner_surface: Entity,
    connection_id: u64,
) -> Result<OrzmaView, &'static str> {
    match kind {
        RegisterKind::Dir {
            root,
            entry,
            interactive,
            forward_keys,
            preload,
        } => {
            let root_path = PathBuf::from(&root);
            if !root_path.is_absolute() || !root_path.is_dir() {
                return Err("invalid_root");
            }
            if !is_safe_entry(&entry) {
                return Err("unsafe_entry");
            }
            Ok(OrzmaView {
                source: OrzmaSource::Dir(root_path),
                entry,
                interactive,
                owner_surface,
                connection_id,
                forward_keys: forward_keys.iter().filter_map(normalize_chord).collect(),
                preload,
                instances: Vec::new(),
            })
        }
        RegisterKind::Inline {
            html,
            interactive,
            forward_keys,
            preload,
        } => {
            if html.len() > MAX_INLINE_HTML {
                return Err("html_too_large");
            }
            Ok(OrzmaView {
                source: OrzmaSource::Inline(html),
                entry: "index.html".into(),
                interactive,
                owner_surface,
                connection_id,
                forward_keys: forward_keys.iter().filter_map(normalize_chord).collect(),
                preload,
                instances: Vec::new(),
            })
        }
        RegisterKind::Url {
            url,
            interactive,
            bridge,
            forward_keys,
            preload,
        } => {
            let url = validate_url_source(&url)?;
            Ok(OrzmaView {
                source: OrzmaSource::Url { url, bridge },
                entry: String::new(),
                interactive,
                owner_surface,
                connection_id,
                forward_keys: forward_keys.iter().filter_map(normalize_chord).collect(),
                preload,
                instances: Vec::new(),
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

/// Validates a `url` register source: parses it, requires an `http`/`https`
/// scheme, then a non-empty host. The scheme check precedes the host check on
/// purpose — `url::Url::parse("javascript:…")` succeeds with no host, so a
/// host-first order would mis-report `javascript:` as `invalid_url` instead of
/// `unsupported_scheme`.
/// Returns the parser-normalized URL (not the raw input) so the validated and
/// loaded forms are identical.
fn validate_url_source(url: &str) -> Result<String, &'static str> {
    let parsed = Url::parse(url).map_err(|_| "invalid_url")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("unsupported_scheme");
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("invalid_url");
    }
    Ok(parsed.into())
}

/// Converts a wire [`HostKeyChord`] to a [`NormalizedChord`], returning `None`
/// for unrecognized key names. Note that `"backtab"` maps to [`KeyCode::Tab`]
/// (the same as `"tab"`): the shift distinction is carried in the modifier bits,
/// so a forward-key `BackTab` and `Tab` are indistinguishable at the host.
fn normalize_chord(chord: &HostKeyChord) -> Option<NormalizedChord> {
    let code = match chord.key.as_str() {
        "tab" | "backtab" => KeyCode::Tab,
        "f1" => KeyCode::F1,
        "f2" => KeyCode::F2,
        "f3" => KeyCode::F3,
        "f4" => KeyCode::F4,
        "f5" => KeyCode::F5,
        "f6" => KeyCode::F6,
        "f7" => KeyCode::F7,
        "f8" => KeyCode::F8,
        "f9" => KeyCode::F9,
        "f10" => KeyCode::F10,
        "f11" => KeyCode::F11,
        "f12" => KeyCode::F12,
        "0" => KeyCode::Digit0,
        "1" => KeyCode::Digit1,
        "2" => KeyCode::Digit2,
        "3" => KeyCode::Digit3,
        "4" => KeyCode::Digit4,
        "5" => KeyCode::Digit5,
        "6" => KeyCode::Digit6,
        "7" => KeyCode::Digit7,
        "8" => KeyCode::Digit8,
        "9" => KeyCode::Digit9,
        "a" => KeyCode::KeyA,
        "b" => KeyCode::KeyB,
        "c" => KeyCode::KeyC,
        "d" => KeyCode::KeyD,
        "e" => KeyCode::KeyE,
        "f" => KeyCode::KeyF,
        "g" => KeyCode::KeyG,
        "h" => KeyCode::KeyH,
        "i" => KeyCode::KeyI,
        "j" => KeyCode::KeyJ,
        "k" => KeyCode::KeyK,
        "l" => KeyCode::KeyL,
        "m" => KeyCode::KeyM,
        "n" => KeyCode::KeyN,
        "o" => KeyCode::KeyO,
        "p" => KeyCode::KeyP,
        "q" => KeyCode::KeyQ,
        "r" => KeyCode::KeyR,
        "s" => KeyCode::KeyS,
        "t" => KeyCode::KeyT,
        "u" => KeyCode::KeyU,
        "v" => KeyCode::KeyV,
        "w" => KeyCode::KeyW,
        "x" => KeyCode::KeyX,
        "y" => KeyCode::KeyY,
        "z" => KeyCode::KeyZ,
        "esc" => KeyCode::Escape,
        " " => KeyCode::Space,
        "down" => KeyCode::ArrowDown,
        "up" => KeyCode::ArrowUp,
        "pagedown" => KeyCode::PageDown,
        "pageup" => KeyCode::PageUp,
        _ => return None,
    };
    let mut alt = false;
    let mut ctrl = false;
    let mut shift = false;
    let mut logo = false;
    for m in &chord.mods {
        match m.as_str() {
            "alt" => alt = true,
            "ctrl" => ctrl = true,
            "shift" => shift = true,
            "meta" => logo = true,
            _ => {}
        }
    }
    Some(NormalizedChord {
        code,
        alt,
        ctrl,
        shift,
        logo,
    })
}

/// Upper bound on a single inline HTML document (4 MiB).
const MAX_INLINE_HTML: usize = 4 * 1024 * 1024;

#[cfg(test)]
mod gc_tests {
    use super::*;

    /// Asserts that the garbage collector drops a view registration once
    /// the surface that owns it is despawned, and leaves it alone while
    /// that surface is alive.
    ///
    /// Case: the user closes a pane whose shell had registered a webview
    /// over the control socket.
    #[test]
    fn gc_purges_registrations_when_owner_surface_despawns() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.add_systems(Update, gc_despawned_surfaces);

        let (handle, _sink) = OrzmaTtyHandle::detached(4, 2);
        let surface = app.world_mut().spawn(handle).id();
        app.world_mut().resource_mut::<OrzmaRegistry>().insert(
            "h0".into(),
            OrzmaView {
                source: OrzmaSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: surface,
                connection_id: 1,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        app.update(); // no despawn yet
        assert!(
            app.world()
                .resource::<OrzmaRegistry>()
                .get(&HandleId::from("h0"))
                .is_some()
        );

        app.world_mut().entity_mut(surface).despawn();
        app.update();
        assert!(
            app.world()
                .resource::<OrzmaRegistry>()
                .get(&HandleId::from("h0"))
                .is_none(),
            "despawning the owner surface purges its registrations"
        );
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;
    use bevy::prelude::Entity;

    #[test]
    fn minted_ids_match_the_osc_view_id_charset() {
        for _ in 0..50 {
            let id = mint_handle_id();
            let id = id.as_str();
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
                "minted id {id} must be lowercase — it is used as an orzma:// host that Chromium lowercases"
            );
        }
    }

    #[test]
    fn minted_ids_are_unique() {
        let a = mint_handle_id();
        let b = mint_handle_id();
        assert_ne!(a, b);
    }

    #[test]
    fn token_registry_binds_and_resolves() {
        let reg = TokenRegistry::default();
        reg.insert("tok", Entity::from_bits(5));
        assert_eq!(reg.resolve("tok"), Some(Entity::from_bits(5)));
        assert_eq!(reg.resolve("nope"), None);
    }

    #[test]
    fn remove_entity_drops_every_key_for_that_surface() {
        let reg = TokenRegistry::default();
        let surface = Entity::from_bits(7);
        reg.insert("%1", surface);
        reg.insert("tok", surface);
        reg.insert("%2", Entity::from_bits(8));
        reg.remove_entity(surface);
        assert_eq!(reg.resolve("%1"), None);
        assert_eq!(reg.resolve("tok"), None);
        assert_eq!(reg.resolve("%2"), Some(Entity::from_bits(8)));
    }

    #[test]
    fn surface_env_is_pure_and_bind_surface_registers() {
        let handle = ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: TokenRegistry::default(),
        };
        let surface = Entity::from_bits(42);
        let token = format!("orzma:{}", surface.to_bits());

        let env = handle.surface_env(surface);
        assert_eq!(
            env,
            [
                ("ORZMA_SOCK".to_string(), "/tmp/ctl.sock".to_string()),
                ("ORZMA_TOKEN".to_string(), token.clone()),
            ]
        );
        assert_eq!(
            handle.tokens.resolve(&token),
            None,
            "surface_env must not register the token (pure read)"
        );

        handle.bind_surface(surface);
        assert_eq!(
            handle.tokens.resolve(&token),
            Some(surface),
            "bind_surface registers token -> surface"
        );
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
        let mut reg = OrzmaRegistry::default();
        reg.insert(HandleId::from("h1"), view(surface(1), 7));
        assert_eq!(
            reg.get(&HandleId::from("h1")).map(|v| v.interactive),
            Some(true)
        );
        assert!(reg.get(&HandleId::from("missing")).is_none());
    }

    #[test]
    fn remove_by_connection_drops_only_that_connections_handles() {
        let mut reg = OrzmaRegistry::default();
        reg.insert(HandleId::from("a"), view(surface(1), 7));
        reg.insert(HandleId::from("b"), view(surface(1), 7));
        reg.insert(HandleId::from("c"), view(surface(2), 8));

        let removed = reg.remove_by_connection(7);
        assert_eq!(removed.len(), 2);
        assert!(reg.get(&HandleId::from("a")).is_none() && reg.get(&HandleId::from("b")).is_none());
        assert!(reg.get(&HandleId::from("c")).is_some());
    }

    #[test]
    fn remove_by_surface_drops_only_that_surfaces_handles() {
        let mut reg = OrzmaRegistry::default();
        reg.insert(HandleId::from("a"), view(surface(1), 7));
        reg.insert(HandleId::from("c"), view(surface(2), 8));

        let removed = reg.remove_by_surface(surface(1));
        assert_eq!(
            removed.iter().map(|r| r.handle.clone()).collect::<Vec<_>>(),
            vec![HandleId::from("a")]
        );
        assert!(reg.get(&HandleId::from("a")).is_none());
        assert!(reg.get(&HandleId::from("c")).is_some());
    }

    #[test]
    fn remove_one_returns_the_owner_surface_when_present() {
        let mut reg = OrzmaRegistry::default();
        reg.insert(HandleId::from("a"), view(surface(3), 9));
        assert_eq!(
            reg.remove(&HandleId::from("a")).map(|r| r.owner_surface),
            Some(surface(3))
        );
        assert!(reg.remove(&HandleId::from("a")).is_none());
    }

    /// Asserts that minting binds an instance to its handle, that the
    /// reverse lookup resolves it, and that releasing the handle takes
    /// every instance with it.
    ///
    /// Case: a program registers one view, asks for a second placement,
    /// then unregisters while both are mounted.
    #[test]
    fn minting_binds_an_instance_to_its_handle_and_release_takes_both() {
        let mut registry = OrzmaRegistry::default();
        // NOTE: Bevy 0.19 has no Entity::from_raw; the repo's other tests use
        // from_bits.
        let owner = Entity::from_bits(7);
        let h = HandleId::from("h");
        registry.insert(h.clone(), view(owner, 1));
        let first = registry.mint_instance(&h).expect("a known handle mints");
        let second = registry.mint_instance(&h).expect("a second mint succeeds");
        assert_ne!(first, second, "each mint draws a distinct id");

        assert_eq!(registry.resolve_instance(first).map(|(h, _)| h), Some(&h));
        assert_eq!(registry.resolve_instance(second).map(|(h, _)| h), Some(&h));
        assert!(registry.mint_instance(&HandleId::from("nope")).is_none());

        let removed = registry.remove(&h).expect("the handle was registered");
        assert_eq!(removed.owner_surface, owner);
        assert_eq!(removed.instances.len(), 2);
        assert!(removed.instances.contains(&first));
        assert!(removed.instances.contains(&second));
        assert!(registry.resolve_instance(first).is_none());
        assert!(registry.resolve_instance(second).is_none());
    }

    fn view(owner: Entity, conn: u64) -> OrzmaView {
        OrzmaView {
            source: OrzmaSource::Dir("/abs".into()),
            entry: "index.html".into(),
            interactive: true,
            owner_surface: owner,
            connection_id: conn,
            forward_keys: vec![],
            preload: vec![],
            instances: Vec::new(),
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
        let orzma_assets = WebviewAssetRegistry::default();
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(orzma_assets.clone()));
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
                    forward_keys: vec![],
                    preload: vec![],
                },
                reply: reply_tx,
            })
            .unwrap();

        app.update();

        let handle = match reply_rx.try_recv().expect("apply must reply") {
            ServerMsg::Registered { handle, .. } => handle,
            other => panic!("unexpected reply: {other:?}"),
        };
        assert!(
            orzma_assets.get(handle.as_str()).is_some(),
            "WebviewAssetRegistry populated for Dir"
        );
        assert!(
            app.world()
                .resource::<OrzmaRegistry>()
                .get(&handle)
                .is_some(),
            "OrzmaRegistry populated"
        );
    }

    #[test]
    fn apply_register_inline_populates_dyn_asset_registry_with_html_bytes() {
        use orzma_webview_host::WebviewAsset;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let orzma_assets = WebviewAssetRegistry::default();
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(orzma_assets.clone()));
        app.add_systems(Update, apply_control_events);

        let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
        ev_tx
            .send(ControlEvent::Register {
                connection_id: 1,
                owner_surface: Entity::from_bits(11),
                kind: RegisterKind::Inline {
                    html: "<h1>x</h1>".into(),
                    interactive: true,
                    forward_keys: vec![],
                    preload: vec![],
                },
                reply: reply_tx,
            })
            .unwrap();

        app.update();

        let handle = match reply_rx.try_recv().expect("apply must reply") {
            ServerMsg::Registered { handle, .. } => handle,
            other => panic!("unexpected reply: {other:?}"),
        };
        assert!(
            matches!(orzma_assets.get(handle.as_str()), Some(WebviewAsset::Inline(bytes)) if bytes == b"<h1>x</h1>"),
            "WebviewAssetRegistry must carry the inline HTML bytes for the minted handle"
        );
    }

    #[test]
    fn apply_register_invalid_root_replies_err() {
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
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
                    forward_keys: vec![],
                    preload: vec![],
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
        let orzma_assets = WebviewAssetRegistry::default();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "h".into(),
            OrzmaView {
                source: OrzmaSource::Dir("/x".into()),
                entry: "i".into(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        orzma_assets.insert_dir("h", "/x".into());
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(orzma_assets.clone()));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Disconnect { connection_id: 5 })
            .unwrap();
        app.update();
        assert!(
            app.world()
                .resource::<OrzmaRegistry>()
                .get(&HandleId::from("h"))
                .is_none()
        );
        assert!(orzma_assets.get("h").is_none());
    }

    #[test]
    fn apply_is_a_noop_when_control_events_missing() {
        let mut app = App::new();
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);
        app.update();
        assert!(app.world().get_resource::<ControlEvents>().is_none());
    }

    #[test]
    fn disconnect_despawns_mounted_webviews_for_its_handles() {
        use crate::webview::mount::Webview;
        let mut app = App::new();
        let orzma_assets = WebviewAssetRegistry::default();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "HMOUNT".into(),
            OrzmaView {
                source: OrzmaSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 9,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let instance = reg
            .mint_instance(&HandleId::from("HMOUNT"))
            .expect("the handle mints");
        let mounted = app
            .world_mut()
            .spawn(Webview {
                handle: "HMOUNT".into(),
                instance,
                slot: 0,
                rows: 10,
                cols: 40,
            })
            .id();
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(WebviewAssetRegistryRes(orzma_assets));
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
                .resource::<OrzmaRegistry>()
                .get(&HandleId::from("HMOUNT"))
                .is_none()
        );
    }

    /// Asserts that releasing a handle hands the placements it had minted
    /// back to the VT on the surface that owned them.
    ///
    /// Case: a program unregisters a view whose placements the terminal
    /// still holds, so their cap slots must be freed without waiting for
    /// the anchors to scroll out of history.
    #[test]
    fn unregister_reclaims_the_released_placements_from_the_vt() {
        #[derive(Resource, Default)]
        struct Reclaimed(Vec<(Entity, Vec<InstanceId>)>);
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let handle = HandleId::from("h");
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            handle.clone(),
            OrzmaView {
                source: OrzmaSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let instance = reg.mint_instance(&handle).expect("the handle mints");
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.init_resource::<Reclaimed>();
        app.add_observer(
            |e: On<RequestTtyWebviewRemove>, mut reclaimed: ResMut<Reclaimed>| {
                reclaimed.0.push((e.terminal, e.instances.clone()));
            },
        );
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Unregister {
                connection_id: 5,
                handle,
            })
            .unwrap();
        app.update();

        assert_eq!(
            app.world().resource::<Reclaimed>().0,
            vec![(surface, vec![instance])],
            "unregister must drop the released placements on the owning surface"
        );
    }

    #[test]
    fn apply_reply_reemits_to_the_originating_webview() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut rpc = OrzmaRpc::default();
        let webview = app.world_mut().spawn_empty().id();
        let g = rpc.mint();
        rpc.note(&g, webview, "p9", 5);
        app.insert_resource(rpc);
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
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
        assert_eq!(channel, "orzma");
        assert_eq!(payload["reqId"], "p9");
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["value"], 99);
    }

    #[test]
    fn apply_reply_from_wrong_connection_does_not_drop_the_pending_call() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut rpc = OrzmaRpc::default();
        let webview = app.world_mut().spawn_empty().id();
        let g = rpc.mint();
        rpc.note(&g, webview, "p9", 5);
        app.insert_resource(rpc);
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
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
            vec![(webview, "orzma".to_string())],
            "only the owning connection's reply settles the page"
        );
    }

    #[test]
    fn apply_emit_is_dropped_for_a_non_bridged_url_view() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "disp".into(),
            OrzmaView {
                source: OrzmaSource::Url {
                    url: "https://example.com".into(),
                    bridge: false,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let instance = reg
            .mint_instance(&HandleId::from("disp"))
            .expect("the handle mints");
        app.world_mut().spawn(Webview {
            handle: "disp".into(),
            instance,
            slot: 0,
            rows: 10,
            cols: 40,
        });
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        #[derive(Resource, Default)]
        struct Caps(Vec<Entity>);
        app.insert_resource(Caps::default());
        app.add_observer(|e: On<HostEmitEvent>, mut c: ResMut<Caps>| c.0.push(e.webview));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Emit {
                connection_id: 5,
                handle: "disp".into(),
                event: "tick".into(),
                payload: serde_json::json!(1),
            })
            .unwrap();
        app.update();

        assert!(
            app.world().resource::<Caps>().0.is_empty(),
            "a non-bridged url view must receive no emit"
        );
    }

    #[test]
    fn apply_register_url_mints_handle_without_dyn_asset() {
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let orzma_assets = WebviewAssetRegistry::default();
        app.insert_resource(OrzmaRegistry::default());
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(orzma_assets.clone()));
        app.add_systems(Update, apply_control_events);

        let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
        ev_tx
            .send(ControlEvent::Register {
                connection_id: 1,
                owner_surface: Entity::from_bits(11),
                kind: RegisterKind::Url {
                    url: "https://example.com".into(),
                    interactive: true,
                    bridge: false,
                    forward_keys: vec![],
                    preload: vec![],
                },
                reply: reply_tx,
            })
            .unwrap();
        app.update();

        let handle = match reply_rx.try_recv().expect("apply must reply") {
            ServerMsg::Registered { handle, .. } => handle,
            other => panic!("unexpected reply: {other:?}"),
        };
        assert!(
            app.world()
                .resource::<OrzmaRegistry>()
                .get(&handle)
                .is_some(),
            "OrzmaRegistry populated"
        );
        assert!(
            orzma_assets.get(handle.as_str()).is_none(),
            "WebviewAssetRegistry must NOT be populated for a url handle"
        );
    }

    #[test]
    fn apply_emit_broadcasts_only_to_owning_connections_handle() {
        use bevy_cef::prelude::HostEmitEvent;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "H".into(),
            OrzmaView {
                source: OrzmaSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: Entity::from_bits(1),
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let instance = reg
            .mint_instance(&HandleId::from("H"))
            .expect("the handle mints");
        let mounted = app
            .world_mut()
            .spawn((
                Webview {
                    handle: "H".into(),
                    instance,
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
                WebviewOwner {
                    connection_id: 5,
                    handle: "H".into(),
                    instance,
                },
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
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
        assert_eq!(caps.0, vec![(mounted, "orzma.event".to_string())]);
    }

    #[test]
    fn apply_navigate_to_updates_webview_source_for_owned_url_view() {
        use bevy_cef::prelude::WebviewSource;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "H".into(),
            OrzmaView {
                source: OrzmaSource::Url {
                    url: "https://example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let instance = reg
            .mint_instance(&HandleId::from("H"))
            .expect("the handle mints");
        let child = app
            .world_mut()
            .spawn((
                Webview {
                    handle: "H".into(),
                    instance,
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
                WebviewSource::new("https://example.com"),
                ChildOf(surface),
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Navigate {
                connection_id: 5,
                owner_surface: surface,
                instance: instance.to_string(),
                action: NavAction::To("https://example.com/next".into()),
            })
            .unwrap();
        app.update();

        match app.world().get::<WebviewSource>(child).unwrap() {
            WebviewSource::Url(u) => assert_eq!(u, "https://example.com/next"),
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn apply_navigate_back_triggers_request_go_back_on_owned_view() {
        use bevy_cef::prelude::{RequestGoBack, WebviewSource};
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "H".into(),
            OrzmaView {
                source: OrzmaSource::Url {
                    url: "https://example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let instance = reg
            .mint_instance(&HandleId::from("H"))
            .expect("the handle mints");
        let child = app
            .world_mut()
            .spawn((
                Webview {
                    handle: "H".into(),
                    instance,
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
                WebviewSource::new("https://example.com"),
                ChildOf(surface),
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        #[derive(Resource, Default)]
        struct BackOn(Vec<Entity>);
        app.insert_resource(BackOn::default());
        app.add_observer(|e: On<RequestGoBack>, mut c: ResMut<BackOn>| c.0.push(e.webview));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Navigate {
                connection_id: 5,
                owner_surface: surface,
                instance: instance.to_string(),
                action: NavAction::Back,
            })
            .unwrap();
        app.update();

        assert_eq!(app.world().resource::<BackOn>().0, vec![child]);
    }

    #[test]
    fn apply_navigate_is_dropped_for_unowned_connection() {
        use bevy_cef::prelude::WebviewSource;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let mut reg = OrzmaRegistry::default();
        reg.insert(
            "H".into(),
            OrzmaView {
                source: OrzmaSource::Url {
                    url: "https://example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        let instance = reg
            .mint_instance(&HandleId::from("H"))
            .expect("the handle mints");
        let child = app
            .world_mut()
            .spawn((
                Webview {
                    handle: "H".into(),
                    instance,
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
                WebviewSource::new("https://example.com"),
                ChildOf(surface),
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OrzmaRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Navigate {
                connection_id: 9, // not the owner (5)
                owner_surface: surface,
                instance: instance.to_string(),
                action: NavAction::To("https://evil.example/x".into()),
            })
            .unwrap();
        app.update();

        match app.world().get::<WebviewSource>(child).unwrap() {
            WebviewSource::Url(u) => {
                assert_eq!(u, "https://example.com", "unowned navigate is a no-op")
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_cef::prelude::FocusedWebview;
    use crossbeam_channel::unbounded;

    fn focus_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<OrzmaRegistry>()
            .init_resource::<OrzmaRpc>()
            .init_resource::<FocusedWebview>()
            .insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app
    }

    /// Registers an inline view owned by `owner_surface` / `connection_id`
    /// and mints its first instance, the way `register` does at runtime.
    fn register_and_mint(
        app: &mut App,
        handle: &str,
        owner_surface: Entity,
        connection_id: u64,
    ) -> InstanceId {
        let handle = HandleId::from(handle);
        let mut registry = app.world_mut().resource_mut::<OrzmaRegistry>();
        registry.insert(
            handle.clone(),
            OrzmaView {
                source: OrzmaSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface,
                connection_id,
                forward_keys: vec![],
                preload: vec![],
                instances: Vec::new(),
            },
        );
        registry.mint_instance(&handle).expect("the handle mints")
    }

    fn spawn_mounted(app: &mut App, surface: Entity, handle: &str, instance: InstanceId) -> Entity {
        app.world_mut()
            .spawn((
                ChildOf(surface),
                Webview {
                    handle: handle.into(),
                    instance,
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
            ))
            .id()
    }

    /// Asserts that a focus op names the mounted child holding that
    /// instance, and that a null instance from the owning surface clears it.
    ///
    /// Case: an app moves keyboard focus into the placement it just
    /// mounted, then hands focus back to the shell.
    #[test]
    fn set_focus_points_focused_webview_at_the_owned_inline_child() {
        let mut app = focus_app();
        let surface = app.world_mut().spawn_empty().id();
        let instance = register_and_mint(&mut app, "h1", surface, 1);
        let child = spawn_mounted(&mut app, surface, "h1", instance);

        let (tx, rx) = unbounded::<ControlEvent>();
        app.insert_resource(ControlEvents(rx));
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface,
            instance: Some(instance.to_string()),
        })
        .unwrap();
        app.world_mut()
            .run_system_once(apply_control_events)
            .unwrap();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "SetFocus must point FocusedWebview at the owned inline child"
        );

        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface,
            instance: None,
        })
        .unwrap();
        app.world_mut()
            .run_system_once(apply_control_events)
            .unwrap();
        assert_eq!(app.world().resource::<FocusedWebview>().0, None);
    }

    /// Asserts that a focus op from a connection that does not own the
    /// instance is dropped even though a focusable child is mounted.
    ///
    /// Case: a second program on the same surface guesses an instance id
    /// another program's registration minted.
    #[test]
    fn set_focus_rejects_an_unowned_instance() {
        let mut app = focus_app();
        let surface = app.world_mut().spawn_empty().id();
        let instance = register_and_mint(&mut app, "h1", surface, 99);
        // Spawn a VALID interactive inline child that WOULD be focused if the
        // ownership check passed. This ensures the guard is the sole gate:
        // deleting the `connection_id` check would let focus be granted and
        // this assertion would FAIL.
        spawn_mounted(&mut app, surface, "h1", instance);

        let (tx, rx) = unbounded::<ControlEvent>();
        app.insert_resource(ControlEvents(rx));
        // connection_id 1 ≠ owner 99 — ownership guard must reject this.
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface,
            instance: Some(instance.to_string()),
        })
        .unwrap();
        app.world_mut()
            .run_system_once(apply_control_events)
            .unwrap();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "focus must be denied when connection_id does not match the registered owner"
        );
    }

    /// Asserts that a blur only clears focus the requesting surface owns,
    /// and does clear it when the owning surface asks.
    ///
    /// Case: a program on a second pane blurs while a webview on the first
    /// pane holds keyboard focus.
    #[test]
    fn blur_does_not_clobber_another_surfaces_focus() {
        let mut app = focus_app();
        let surface_a = app.world_mut().spawn_empty().id();
        let surface_b = app.world_mut().spawn_empty().id();

        let instance = register_and_mint(&mut app, "ha", surface_a, 1);
        let child_a = spawn_mounted(&mut app, surface_a, "ha", instance);

        let (tx, rx) = unbounded::<ControlEvent>();
        app.insert_resource(ControlEvents(rx));

        // Focus the surface_a child via the owning connection.
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface_a,
            instance: Some(instance.to_string()),
        })
        .unwrap();
        app.world_mut()
            .run_system_once(apply_control_events)
            .unwrap();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child_a),
            "focus must land on the surface_a child after the SetFocus from connection 1"
        );

        // Blur from a DIFFERENT surface (surface_b / connection 2) must NOT
        // clear the focus that belongs to surface_a.
        tx.send(ControlEvent::SetFocus {
            connection_id: 2,
            owner_surface: surface_b,
            instance: None,
        })
        .unwrap();
        app.world_mut()
            .run_system_once(apply_control_events)
            .unwrap();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child_a),
            "blur from surface_b must not clobber surface_a's active focus"
        );

        // Blur from the OWNING side (surface_a / connection 1) must clear it.
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface_a,
            instance: None,
        })
        .unwrap();
        app.world_mut()
            .run_system_once(apply_control_events)
            .unwrap();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "blur from the owning surface must clear focus"
        );
    }
}

#[cfg(test)]
mod back_channel_state_tests {
    use super::*;
    use bevy::prelude::Entity;

    #[test]
    fn orzma_rpc_take_for_connection_matches_only_the_owning_connection() {
        let mut rpc = OrzmaRpc::default();
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
    fn orzma_rpc_drains_a_connections_inflight() {
        let mut rpc = OrzmaRpc::default();
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
    fn orzma_rpc_drain_webview_drops_only_that_webviews_calls() {
        let mut rpc = OrzmaRpc::default();
        let a = rpc.mint();
        let b = rpc.mint();
        rpc.note(&a, Entity::from_bits(1), "p", 5);
        rpc.note(&b, Entity::from_bits(2), "p", 5);
        rpc.drain_webview(Entity::from_bits(1));
        assert!(rpc.take_for_connection(&a, 5).is_none());
        assert!(rpc.take_for_connection(&b, 5).is_some());
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::*;
    use crate::control_plane::protocol::HostKeyChord;

    #[test]
    fn normalize_chord_maps_keys_and_mods() {
        let n = normalize_chord(&HostKeyChord {
            mods: vec!["alt".into()],
            key: "h".into(),
        })
        .unwrap();
        assert_eq!(n.code, KeyCode::KeyH);
        assert!(n.alt && !n.ctrl && !n.shift && !n.logo);
        assert_eq!(
            normalize_chord(&HostKeyChord {
                mods: vec![],
                key: "f5".into()
            })
            .unwrap()
            .code,
            KeyCode::F5
        );
        assert_eq!(
            normalize_chord(&HostKeyChord {
                mods: vec![],
                key: "tab".into()
            })
            .unwrap()
            .code,
            KeyCode::Tab
        );
        assert!(
            normalize_chord(&HostKeyChord {
                mods: vec![],
                key: "nope".into()
            })
            .is_none()
        );
    }

    #[test]
    fn normalize_chord_maps_forward_keys_keys() {
        let cases: &[(&str, KeyCode)] = &[
            ("esc", KeyCode::Escape),
            (" ", KeyCode::Space),
            ("down", KeyCode::ArrowDown),
            ("up", KeyCode::ArrowUp),
            ("pagedown", KeyCode::PageDown),
            ("pageup", KeyCode::PageUp),
        ];
        for (key, expected) in cases {
            let chord = normalize_chord(&HostKeyChord {
                mods: vec![],
                key: (*key).into(),
            });
            assert_eq!(
                chord.map(|c| c.code),
                Some(*expected),
                "failed for key={key:?}"
            );
        }
    }
}

#[cfg(test)]
mod url_source_tests {
    use super::*;
    use bevy::prelude::Entity;

    #[test]
    fn validate_url_source_accepts_http_and_https() {
        assert_eq!(
            validate_url_source("https://example.com").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            validate_url_source("http://localhost:3000/x").unwrap(),
            "http://localhost:3000/x"
        );
    }

    #[test]
    fn validate_url_source_returns_the_normalized_url() {
        assert_eq!(
            validate_url_source("  https://example.com  ").unwrap(),
            "https://example.com/"
        );
    }

    #[test]
    fn validate_url_source_rejects_non_web_schemes_as_unsupported() {
        assert_eq!(
            validate_url_source("file:///etc/passwd"),
            Err("unsupported_scheme")
        );
        assert_eq!(
            validate_url_source("javascript:alert(1)"),
            Err("unsupported_scheme")
        );
        assert_eq!(
            validate_url_source("data:text/html,<h1>x</h1>"),
            Err("unsupported_scheme")
        );
        assert_eq!(
            validate_url_source("orzma://h/index.html"),
            Err("unsupported_scheme")
        );
    }

    #[test]
    fn validate_url_source_rejects_garbage_as_invalid() {
        assert_eq!(validate_url_source("not a url"), Err("invalid_url"));
        assert_eq!(validate_url_source(""), Err("invalid_url"));
        assert_eq!(validate_url_source("http://"), Err("invalid_url"));
    }

    #[test]
    fn build_view_url_accepts_https_and_carries_bridge() {
        let v = build_view(
            RegisterKind::Url {
                url: "https://example.com".into(),
                interactive: true,
                bridge: true,
                forward_keys: vec![],
                preload: vec![],
            },
            Entity::from_bits(1),
            7,
        )
        .expect("https accepted");
        assert!(matches!(
            v.source,
            OrzmaSource::Url { ref url, bridge: true } if url == "https://example.com/"
        ));
    }

    #[test]
    fn build_view_url_rejects_file_scheme() {
        let err = build_view(
            RegisterKind::Url {
                url: "file:///etc/passwd".into(),
                interactive: true,
                bridge: false,
                forward_keys: vec![],
                preload: vec![],
            },
            Entity::from_bits(1),
            7,
        )
        .unwrap_err();
        assert_eq!(err, "unsupported_scheme");
    }

    #[test]
    fn build_view_copies_preload_through() {
        let view = build_view(
            RegisterKind::Inline {
                html: "<h1>x</h1>".into(),
                interactive: true,
                forward_keys: vec![],
                preload: vec!["window.A=1;".into()],
            },
            Entity::from_bits(1),
            1,
        )
        .expect("valid inline");
        assert_eq!(view.preload, vec!["window.A=1;".to_string()]);
    }
}
