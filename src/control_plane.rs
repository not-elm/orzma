//! Native ozmux control plane: a local Unix-socket listener that accepts
//! authenticated dynamic webview registrations (Tier 1) from local programs,
//! mints opaque handles into the `DynamicRegistry`, and tears them down on
//! disconnect or surface despawn. Mirrors the Tokio-free thread model of
//! `ozmux_extension_host::rpc_client`.

use bevy::prelude::*;
use data_encoding::BASE32_NOPAD;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

mod protocol;

/// Where a dynamic view's content lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DynSource {
    /// Files served under this absolute root via `ozmux-dyn://`.
    Dir(PathBuf),
    /// A single inline HTML document served via `WebviewSource::InlineHtml`.
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
        let drained: Vec<String> = self
            .by_handle
            .iter()
            .filter(|(_, v)| v.connection_id == connection_id)
            .map(|(h, _)| h.clone())
            .collect();
        for h in &drained {
            self.by_handle.remove(h);
        }
        drained
    }

    /// Removes every handle owned by `owner_surface`, returning the removed
    /// handles (so the caller can purge the `DynAssetRegistry` too).
    pub(crate) fn remove_by_surface(&mut self, owner_surface: Entity) -> Vec<String> {
        let drained: Vec<String> = self
            .by_handle
            .iter()
            .filter(|(_, v)| v.owner_surface == owner_surface)
            .map(|(h, _)| h.clone())
            .collect();
        for h in &drained {
            self.by_handle.remove(h);
        }
        drained
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

    /// Removes a token binding.
    pub(crate) fn remove(&self, token: &str) {
        self.0.write().unwrap().remove(token);
    }
}

/// Mints an opaque 128-bit identifier (CSPRNG), base32-encoded (unpadded). The
/// alphabet `A-Z2-7` is a subset of the OSC `view_id` charset
/// `^[A-Za-z0-9._-]{1,128}$`, so a minted handle is a valid `mount-inline;<id>`.
fn mint_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG is available");
    BASE32_NOPAD.encode(&bytes)
}

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
