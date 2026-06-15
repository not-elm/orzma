//! `ozma-dyn://<handle>/<path>` custom-scheme handler for Tier 1 dynamic
//! webviews. Resolves `<handle>` to a registered asset (`Dir` root or inline
//! HTML bytes) via a shared `DynAssetRegistry` and serves files through
//! `serve_static_asset` or directly from memory. Behind the `cef` feature.

#[cfg(feature = "cef")]
use crate::asset::{AssetOutcome, serve_static_asset};
#[cfg(feature = "cef")]
use bevy_cef_core::prelude::{
    CefCustomScheme, CefSchemeBody, CefSchemeHandler, CefSchemeOptions, CefSchemeRequest,
    CefSchemeResponse,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// The content backing one dynamic handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynAsset {
    /// Files served under this absolute root directory.
    Dir(PathBuf),
    /// A single inline HTML document served from memory.
    Inline(Vec<u8>),
}

/// A shared, interior-mutable map of dynamic `handle → DynAsset` for
/// Tier 1 dynamic webview registrations. The CEF scheme handler is constructed
/// at `CefPlugin::build()` and reads handles registered after its construction.
#[derive(Clone, Default)]
pub struct DynAssetRegistry(Arc<RwLock<HashMap<String, DynAsset>>>);

impl DynAssetRegistry {
    /// Returns (cloning) the asset for `handle`, if registered.
    pub fn get(&self, handle: &str) -> Option<DynAsset> {
        self.0.read().unwrap().get(handle).cloned()
    }

    /// Inserts/replaces an on-disk directory root for `handle`.
    pub fn insert_dir(&self, handle: impl Into<String>, root: PathBuf) {
        self.0
            .write()
            .unwrap()
            .insert(handle.into(), DynAsset::Dir(root));
    }

    /// Inserts/replaces inline HTML bytes for `handle`.
    pub fn insert_inline(&self, handle: impl Into<String>, html: Vec<u8>) {
        self.0
            .write()
            .unwrap()
            .insert(handle.into(), DynAsset::Inline(html));
    }

    /// Removes `handle`, if present.
    pub fn remove(&self, handle: &str) {
        self.0.write().unwrap().remove(handle);
    }
}

/// Parses `ozma-dyn://<handle>/<path>[?query]` into `(handle, path)`; strips
/// the query/fragment and defaults an empty path to `"index.html"`. Returns
/// `None` unless it is a well-formed `ozma-dyn://` URL with a non-empty handle.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn parse_dyn_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("ozma-dyn://")?;
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

/// The resolved outcome of an `ozma-dyn://` URL lookup.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
enum ResolvedDyn<'a> {
    /// Serve files from this directory root; `path` is the relative file path.
    Dir { root: PathBuf, path: &'a str },
    /// Serve these inline HTML bytes directly from memory.
    Inline(Vec<u8>),
}

/// Resolves an `ozma-dyn://<handle>/<path>` URL via the registry, or `Err(404)`
/// for an unknown or unparseable handle.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn resolve_request<'a>(registry: &DynAssetRegistry, url: &'a str) -> Result<ResolvedDyn<'a>, u16> {
    let (handle, path) = parse_dyn_url(url).ok_or(404u16)?;
    match registry.get(handle).ok_or(404u16)? {
        DynAsset::Dir(root) => Ok(ResolvedDyn::Dir { root, path }),
        // NOTE: an inline registration is a single self-contained document served
        // at the canonical entry only. A relative subresource request gets 404
        // rather than the document body under a mismatched MIME type — use a Dir
        // registration for multi-file content.
        DynAsset::Inline(html) if path == "index.html" => Ok(ResolvedDyn::Inline(html)),
        DynAsset::Inline(_) => Err(404),
    }
}

/// Returns the bare media type (drops any `;`-delimited parameters) for CEF's
/// `mime_type` field, flooring an empty/blank input to `application/octet-stream`.
/// CEF expects a bare type (e.g. `text/html`); a full `Content-Type` value with
/// parameters (`text/html; charset=utf-8`) is not recognized, so Chromium fails
/// to classify the document and renders blank. An empty `mime_type` triggers the
/// same blank render, so it is floored to `application/octet-stream` (matching
/// the SDK file handler's default) rather than passed through empty.
#[cfg(feature = "cef")]
fn bare_mime(content_type: &str) -> String {
    let bare = content_type.split(';').next().unwrap_or("").trim();
    if bare.is_empty() {
        "application/octet-stream".to_string()
    } else {
        bare.to_string()
    }
}

/// A minimal text `CefSchemeResponse` for error statuses (bevy_cef provides only
/// `not_found()` / `bytes()`).
#[cfg(feature = "cef")]
fn status_text(status: u16, msg: &str) -> CefSchemeResponse {
    CefSchemeResponse {
        status,
        mime_type: "text/plain".into(),
        headers: Vec::new(),
        body: CefSchemeBody::Bytes(msg.as_bytes().to_vec()),
    }
}

/// The custom scheme name registered with CEF for dynamic Tier 1 webviews.
#[cfg(feature = "cef")]
pub const SCHEME_NAME: &str = "ozma-dyn";

/// Serves `ozma-dyn://<handle>/<path>` by dispatching `<handle>` through a
/// shared `DynAssetRegistry` to `serve_static_asset` (Dir) or memory (Inline).
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
        match resolve_request(&self.registry, &request.url) {
            Err(_) => {
                bevy::log::debug!(
                    url = %request.url,
                    "ozma-dyn request unresolved (unknown handle or non-index inline path) -> 404"
                );
                CefSchemeResponse::not_found()
            }
            Ok(ResolvedDyn::Inline(html)) => {
                bevy::log::debug!(
                    url = %request.url,
                    bytes = html.len(),
                    "ozma-dyn inline html served"
                );
                CefSchemeResponse {
                    status: 200,
                    mime_type: "text/html".to_string(),
                    headers: Vec::new(),
                    body: CefSchemeBody::Bytes(html),
                }
            }
            Ok(ResolvedDyn::Dir { root, path }) => match serve_static_asset(&root, path) {
                AssetOutcome::Ok { content_type, body } => {
                    let mime = bare_mime(&content_type);
                    bevy::log::debug!(
                        url = %request.url,
                        mime = %mime,
                        bytes = body.len(),
                        "ozma-dyn static asset served"
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
            },
        }
    }
}

/// Builds the `ozma-dyn` scheme registration to pass to `CefPlugin`, dispatching
/// every `ozma-dyn://<handle>/…` URL through the shared `DynAssetRegistry`.
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
            parse_dyn_url("ozma-dyn://abc/index.html"),
            Some(("abc", "index.html"))
        );
        assert_eq!(
            parse_dyn_url("ozma-dyn://abc/sub/app.js"),
            Some(("abc", "sub/app.js"))
        );
    }

    #[test]
    fn empty_path_defaults_to_index() {
        assert_eq!(
            parse_dyn_url("ozma-dyn://abc/"),
            Some(("abc", "index.html"))
        );
        assert_eq!(
            parse_dyn_url("ozma-dyn://abc"),
            Some(("abc", "index.html"))
        );
    }

    #[test]
    fn strips_query_and_fragment() {
        assert_eq!(
            parse_dyn_url("ozma-dyn://abc/a.js?v=2"),
            Some(("abc", "a.js"))
        );
        assert_eq!(
            parse_dyn_url("ozma-dyn://abc/a.js#section"),
            Some(("abc", "a.js"))
        );
    }

    #[test]
    fn rejects_foreign_or_empty_handle() {
        assert_eq!(parse_dyn_url("ozmux-ext://abc/x"), None);
        assert_eq!(parse_dyn_url("ozma-dyn:///x"), None);
    }

    #[test]
    fn registry_holds_dir_and_inline_variants() {
        let reg = DynAssetRegistry::default();
        reg.insert_dir("d1", PathBuf::from("/abs/ui"));
        reg.insert_inline("i1", b"<h1>hi</h1>".to_vec());
        assert!(
            matches!(reg.get("d1"), Some(DynAsset::Dir(p)) if *p == *std::path::Path::new("/abs/ui"))
        );
        assert!(matches!(reg.get("i1"), Some(DynAsset::Inline(b)) if b == b"<h1>hi</h1>"));
        assert!(reg.get("missing").is_none());
        reg.remove("i1");
        assert!(reg.get("i1").is_none());
    }

    #[test]
    fn resolve_request_returns_inline_bytes_as_html() {
        let reg = DynAssetRegistry::default();
        reg.insert_inline("i1", b"<h1>hi</h1>".to_vec());
        match resolve_request(&reg, "ozma-dyn://i1/index.html").expect("registered") {
            ResolvedDyn::Inline(html) => assert_eq!(html, b"<h1>hi</h1>"),
            ResolvedDyn::Dir { .. } => panic!("expected inline"),
        }
    }

    #[test]
    fn inline_404s_subresource_paths_other_than_the_index() {
        let reg = DynAssetRegistry::default();
        reg.insert_inline("i1", b"<h1>hi</h1>".to_vec());
        assert!(resolve_request(&reg, "ozma-dyn://i1/").is_ok());
        assert!(resolve_request(&reg, "ozma-dyn://i1/index.html").is_ok());
        assert_eq!(
            resolve_request(&reg, "ozma-dyn://i1/app.js").err(),
            Some(404)
        );
        assert_eq!(
            resolve_request(&reg, "ozma-dyn://i1/logo.png").err(),
            Some(404)
        );
    }

    #[test]
    fn resolves_registered_dir_and_404s_unknown() {
        let reg = DynAssetRegistry::default();
        assert_eq!(
            resolve_request(&reg, "ozma-dyn://ghost/index.html").err(),
            Some(404)
        );
        reg.insert_dir("h1", PathBuf::from("/abs/ui"));
        match resolve_request(&reg, "ozma-dyn://h1/app.js").expect("registered") {
            ResolvedDyn::Dir { root, path } => {
                assert_eq!(root, PathBuf::from("/abs/ui"));
                assert_eq!(path, "app.js");
            }
            ResolvedDyn::Inline(_) => panic!("expected dir"),
        }
    }

    #[test]
    fn remove_drops_the_handle() {
        let reg = DynAssetRegistry::default();
        reg.insert_dir("h1", PathBuf::from("/abs/ui"));
        reg.remove("h1");
        assert_eq!(
            resolve_request(&reg, "ozma-dyn://h1/index.html").err(),
            Some(404)
        );
    }

    #[cfg(feature = "cef")]
    #[test]
    fn bare_mime_strips_charset_parameter() {
        assert_eq!(bare_mime("text/html; charset=utf-8"), "text/html");
        assert_eq!(
            bare_mime("text/javascript; charset=utf-8"),
            "text/javascript"
        );
        assert_eq!(bare_mime("application/wasm"), "application/wasm");
    }

    #[cfg(feature = "cef")]
    #[test]
    fn bare_mime_floors_empty_to_octet_stream() {
        assert_eq!(bare_mime(""), "application/octet-stream");
        assert_eq!(bare_mime("   "), "application/octet-stream");
        assert_eq!(bare_mime("; charset=utf-8"), "application/octet-stream");
    }
}
