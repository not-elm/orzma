//! `ozmux-dyn://<handle>/<path>` custom-scheme handler for Tier 1 dynamic
//! webviews. Resolves `<handle>` to a registered asset root via a shared
//! `DynAssetRegistry` and serves files through `serve_static_asset`, reusing
//! the `ozmux-ext` MIME/error helpers. Behind the `cef` feature.

#[cfg(feature = "cef")]
use crate::asset::{AssetOutcome, serve_static_asset};
#[cfg(feature = "cef")]
use crate::scheme::{bare_mime, status_text};
#[cfg(feature = "cef")]
use bevy_cef_core::prelude::{
    CefCustomScheme, CefSchemeBody, CefSchemeHandler, CefSchemeOptions, CefSchemeRequest,
    CefSchemeResponse,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// A shared, interior-mutable map of dynamic `handle → on-disk asset root` for
/// `Dir`-source Tier 1 registrations. The CEF scheme handler is constructed at
/// `CefPlugin::build()` and reads handles registered after its construction,
/// mirroring `AssetSourceRegistry`.
#[derive(Clone, Default)]
pub struct DynAssetRegistry(Arc<RwLock<HashMap<String, PathBuf>>>);

impl DynAssetRegistry {
    /// Returns (cloning) the asset root for `handle`, if registered.
    pub fn get(&self, handle: &str) -> Option<PathBuf> {
        self.0.read().unwrap().get(handle).cloned()
    }

    /// Inserts/replaces the asset root for `handle`.
    pub fn insert(&self, handle: impl Into<String>, root: PathBuf) {
        self.0.write().unwrap().insert(handle.into(), root);
    }

    /// Removes `handle`, if present.
    pub fn remove(&self, handle: &str) {
        self.0.write().unwrap().remove(handle);
    }
}

/// Parses `ozmux-dyn://<handle>/<path>[?query]` into `(handle, path)`; strips
/// the query/fragment and defaults an empty path to `"index.html"`. Returns
/// `None` unless it is a well-formed `ozmux-dyn://` URL with a non-empty handle.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn parse_dyn_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("ozmux-dyn://")?;
    let rest = rest
        .split_once(['?', '#'])
        .map_or(rest, |(before, _)| before);
    let (handle, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    if handle.is_empty() {
        return None;
    }
    let path = if path.is_empty() { "index.html" } else { path };
    Some((handle, path))
}

/// Resolves an `ozmux-dyn://<handle>/<path>` URL to `(root, path)` via the
/// registry, or `Err(404)` for an unknown/unparseable handle.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn resolve_request<'a>(
    registry: &DynAssetRegistry,
    url: &'a str,
) -> Result<(PathBuf, &'a str), u16> {
    let (handle, path) = parse_dyn_url(url).ok_or(404u16)?;
    let root = registry.get(handle).ok_or(404u16)?;
    Ok((root, path))
}

/// The custom scheme name registered with CEF for dynamic Tier 1 webviews.
#[cfg(feature = "cef")]
pub const SCHEME_NAME: &str = "ozmux-dyn";

/// Serves `ozmux-dyn://<handle>/<path>` by dispatching `<handle>` through a
/// shared `DynAssetRegistry` to `serve_static_asset`.
#[cfg(feature = "cef")]
struct OzmuxDynScheme {
    registry: DynAssetRegistry,
}

#[cfg(feature = "cef")]
impl OzmuxDynScheme {
    fn new(registry: DynAssetRegistry) -> Self {
        Self { registry }
    }
}

#[cfg(feature = "cef")]
impl CefSchemeHandler for OzmuxDynScheme {
    fn handle(&self, request: &CefSchemeRequest) -> CefSchemeResponse {
        let (root, path) = match resolve_request(&self.registry, &request.url) {
            Ok(resolved) => resolved,
            Err(_) => return CefSchemeResponse::not_found(),
        };
        match serve_static_asset(&root, path) {
            AssetOutcome::Ok { content_type, body } => {
                let mime = bare_mime(&content_type);
                bevy::log::debug!(
                    url = %request.url,
                    mime = %mime,
                    bytes = body.len(),
                    "ozmux-dyn static asset served"
                );
                CefSchemeResponse {
                    status: 200,
                    mime_type: mime,
                    headers: Vec::new(),
                    body: CefSchemeBody::Bytes(body),
                }
            }
            AssetOutcome::NotFound => CefSchemeResponse::not_found(),
            AssetOutcome::Forbidden => status_text(403, "forbidden asset path"),
            AssetOutcome::TooLarge => status_text(413, "asset too large"),
        }
    }
}

/// Builds the `ozmux-dyn` scheme registration to pass to `CefPlugin`, dispatching
/// every `ozmux-dyn://<handle>/…` URL through the shared `DynAssetRegistry`.
#[cfg(feature = "cef")]
pub fn custom_dyn_scheme(registry: DynAssetRegistry) -> CefCustomScheme {
    CefCustomScheme {
        name: SCHEME_NAME.to_string(),
        options: CefSchemeOptions::STANDARD
            | CefSchemeOptions::SECURE
            | CefSchemeOptions::CORS_ENABLED
            | CefSchemeOptions::FETCH_ENABLED
            | CefSchemeOptions::DISPLAY_ISOLATED,
        domain: None,
        handler: Arc::new(OzmuxDynScheme::new(registry)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_handle_and_path() {
        assert_eq!(
            parse_dyn_url("ozmux-dyn://abc/index.html"),
            Some(("abc", "index.html"))
        );
        assert_eq!(
            parse_dyn_url("ozmux-dyn://abc/sub/app.js"),
            Some(("abc", "sub/app.js"))
        );
    }

    #[test]
    fn empty_path_defaults_to_index() {
        assert_eq!(
            parse_dyn_url("ozmux-dyn://abc/"),
            Some(("abc", "index.html"))
        );
        assert_eq!(
            parse_dyn_url("ozmux-dyn://abc"),
            Some(("abc", "index.html"))
        );
    }

    #[test]
    fn strips_query_and_fragment() {
        assert_eq!(
            parse_dyn_url("ozmux-dyn://abc/a.js?v=2"),
            Some(("abc", "a.js"))
        );
        assert_eq!(
            parse_dyn_url("ozmux-dyn://abc/a.js#section"),
            Some(("abc", "a.js"))
        );
    }

    #[test]
    fn rejects_foreign_or_empty_handle() {
        assert_eq!(parse_dyn_url("ozmux-ext://abc/x"), None);
        assert_eq!(parse_dyn_url("ozmux-dyn:///x"), None);
    }

    #[test]
    fn resolves_registered_handle_and_404s_unknown() {
        let reg = DynAssetRegistry::default();
        assert_eq!(
            resolve_request(&reg, "ozmux-dyn://ghost/index.html").err(),
            Some(404)
        );
        reg.insert("h1", PathBuf::from("/abs/ui"));
        let (root, path) = resolve_request(&reg, "ozmux-dyn://h1/app.js").expect("registered");
        assert_eq!(root, PathBuf::from("/abs/ui"));
        assert_eq!(path, "app.js");
    }

    #[test]
    fn remove_drops_the_handle() {
        let reg = DynAssetRegistry::default();
        reg.insert("h1", PathBuf::from("/abs/ui"));
        reg.remove("h1");
        assert_eq!(
            resolve_request(&reg, "ozmux-dyn://h1/index.html").err(),
            Some(404)
        );
    }
}
