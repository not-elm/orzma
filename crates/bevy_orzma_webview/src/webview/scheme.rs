//! `orzma://<handle>/<path>` custom-scheme handler for Tier 1 dynamic
//! webviews: `<handle>` resolves through a shared `WebviewAssetRegistry` to a
//! directory root or to inline HTML bytes.

use crate::error::{WebviewError, WebviewResult};
use crate::webview::scheme::asset::StaticAsset;
use bevy_cef_core::prelude::{
    CefCustomScheme, CefSchemeBody, CefSchemeHandler, CefSchemeOptions, CefSchemeRequest,
    CefSchemeResponse,
};
use orzma_webview_host::prelude::WebviewAsset;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

mod asset;

/// A shared, interior-mutable map of dynamic `handle → WebviewAsset` for
/// Tier 1 dynamic webview registrations. Every clone sees the same handles,
/// and a thread that panicked while holding the lock does not stop the
/// others from reading or writing it.
#[derive(Clone, Default)]
pub struct WebviewAssetRegistry(Arc<RwLock<HashMap<String, WebviewAsset>>>);

impl WebviewAssetRegistry {
    /// Returns (cloning) the asset for `handle`, if registered.
    pub fn get(&self, handle: &str) -> Option<WebviewAsset> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(handle)
            .cloned()
    }

    /// Inserts/replaces an on-disk directory root for `handle`.
    pub fn insert_dir(&self, handle: impl Into<String>, root: PathBuf) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(handle.into(), WebviewAsset::Dir(root));
    }

    /// Inserts/replaces inline HTML bytes for `handle`.
    pub fn insert_inline(&self, handle: impl Into<String>, html: Vec<u8>) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(handle.into(), WebviewAsset::Inline(html));
    }

    /// Removes `handle`, if present.
    pub fn remove(&self, handle: &str) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(handle);
    }
}

/// Builds the `orzma` scheme registration to pass to `CefPlugin`, dispatching
/// every `orzma://<handle>/…` URL through the shared `WebviewAssetRegistry`.
pub(crate) fn custom_orzma_scheme(registry: WebviewAssetRegistry) -> CefCustomScheme {
    /// The custom scheme name registered with CEF for dynamic Tier 1 webviews.
    const SCHEME_NAME: &str = "orzma";

    CefCustomScheme {
        name: SCHEME_NAME.to_string(),
        options: CefSchemeOptions::STANDARD
            | CefSchemeOptions::SECURE
            | CefSchemeOptions::CORS_ENABLED
            | CefSchemeOptions::FETCH_ENABLED
            | CefSchemeOptions::DISPLAY_ISOLATED,
        domain: None,
        handler: Arc::new(OrzmaScheme::new(registry)),
    }
}

/// Parses `orzma://<handle>/<path>[?query]` into `(handle, path)`; strips
/// the query/fragment and defaults an empty path to `"index.html"`. Returns
/// `None` unless it is a well-formed `orzma://` URL with a non-empty handle.
fn parse_orzma_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("orzma://")?;
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

/// The resolved outcome of an `orzma://` URL lookup.
enum ResolvedOrzma<'a> {
    /// Serve files from this directory root; `path` is the relative file path.
    Dir { root: PathBuf, path: &'a str },
    /// Serve these inline HTML bytes directly from memory.
    Inline(Vec<u8>),
}

impl<'a> ResolvedOrzma<'a> {
    /// Resolves an `orzma://<handle>/<path>` URL via the registry. An inline
    /// registration answers only its `index.html` entry.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewError::UnknownAsset`] for a malformed URL, an
    /// unregistered handle, or any other path of an inline registration.
    fn resolve(registry: &WebviewAssetRegistry, url: &'a str) -> WebviewResult<Self> {
        let (handle, path) = parse_orzma_url(url).ok_or(WebviewError::UnknownAsset)?;
        match registry.get(handle).ok_or(WebviewError::UnknownAsset)? {
            WebviewAsset::Dir(root) => Ok(Self::Dir { root, path }),
            WebviewAsset::Inline(html) if path == "index.html" => Ok(Self::Inline(html)),
            WebviewAsset::Inline(_) => Err(WebviewError::UnknownAsset),
        }
    }
}

/// Returns the bare media type (drops any `;`-delimited parameters) for CEF's
/// `mime_type` field, flooring an empty or blank input to
/// `application/octet-stream`.
fn bare_mime(content_type: &str) -> String {
    let bare = content_type.split(';').next().unwrap_or("").trim();
    if bare.is_empty() {
        "application/octet-stream".to_string()
    } else {
        bare.to_string()
    }
}

/// The response an `orzma://` request that failed with the error gets.
impl From<WebviewError> for CefSchemeResponse {
    fn from(error: WebviewError) -> Self {
        match error.http_status() {
            404 => Self::not_found(),
            status => Self {
                status,
                mime_type: "text/plain".into(),
                headers: Vec::new(),
                body: CefSchemeBody::Bytes(error.to_string().into_bytes()),
            },
        }
    }
}

/// Serves `orzma://<handle>/<path>` by dispatching `<handle>` through a
/// shared `WebviewAssetRegistry` to `StaticAsset::read` (Dir) or memory (Inline).
struct OrzmaScheme {
    registry: WebviewAssetRegistry,
}

impl OrzmaScheme {
    fn new(registry: WebviewAssetRegistry) -> Self {
        Self { registry }
    }

    /// The successful response for `url`.
    fn respond(&self, url: &str) -> WebviewResult<CefSchemeResponse> {
        match ResolvedOrzma::resolve(&self.registry, url)? {
            ResolvedOrzma::Inline(html) => {
                tracing::debug!(url, bytes = html.len(), "orzma inline html served");
                Ok(CefSchemeResponse::bytes("text/html", html))
            }
            ResolvedOrzma::Dir { root, path } => {
                let asset = StaticAsset::read(&root, path)?;
                let mime = bare_mime(asset.content_type());
                let body = asset.into_body();
                tracing::debug!(url, mime = %mime, bytes = body.len(), "orzma static asset served");
                Ok(CefSchemeResponse::bytes(mime, body))
            }
        }
    }
}

impl CefSchemeHandler for OrzmaScheme {
    fn handle(&self, request: &CefSchemeRequest) -> CefSchemeResponse {
        match self.respond(&request.url) {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!(url = %request.url, %error, "orzma request refused");
                CefSchemeResponse::from(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::thread;

    /// Asserts that an `orzma://` URL splits into its handle and its path.
    ///
    /// Case: a directory page requests its entry and a nested script.
    #[test]
    fn parses_handle_and_path() {
        assert_eq!(
            parse_orzma_url("orzma://abc/index.html"),
            Some(("abc", "index.html"))
        );
        assert_eq!(
            parse_orzma_url("orzma://abc/sub/app.js"),
            Some(("abc", "sub/app.js"))
        );
    }

    /// Asserts that a URL with no path, or only a slash, resolves to
    /// `index.html`.
    ///
    /// Case: CEF loads the bare origin of a registration.
    #[test]
    fn an_empty_path_defaults_to_the_index() {
        assert_eq!(parse_orzma_url("orzma://abc/"), Some(("abc", "index.html")));
        assert_eq!(parse_orzma_url("orzma://abc"), Some(("abc", "index.html")));
    }

    /// Asserts that the query and the fragment are not part of the path.
    ///
    /// Case: a page loads a cache-busted script and follows an in-page
    /// anchor.
    #[test]
    fn the_query_and_fragment_are_stripped() {
        assert_eq!(
            parse_orzma_url("orzma://abc/a.js?v=2"),
            Some(("abc", "a.js"))
        );
        assert_eq!(
            parse_orzma_url("orzma://abc/a.js#section"),
            Some(("abc", "a.js"))
        );
    }

    /// Asserts that another scheme and an empty handle do not parse.
    ///
    /// Case: a page requests a look-alike scheme and an origin with no host.
    #[test]
    fn a_foreign_scheme_or_empty_handle_does_not_parse() {
        assert_eq!(parse_orzma_url("orzma-ext://abc/x"), None);
        assert_eq!(parse_orzma_url("orzma:///x"), None);
    }

    /// Asserts that the registry holds directory and inline assets under
    /// their handles and forgets a removed one.
    ///
    /// Case: one program registers a bundle directory and another an inline
    /// document, and the second unregisters.
    #[test]
    fn the_registry_holds_dir_and_inline_assets() {
        let reg = WebviewAssetRegistry::default();
        reg.insert_dir("d1", PathBuf::from("/abs/ui"));
        reg.insert_inline("i1", b"<h1>hi</h1>".to_vec());
        assert_eq!(
            reg.get("d1"),
            Some(WebviewAsset::Dir(Path::new("/abs/ui").to_path_buf()))
        );
        assert_eq!(
            reg.get("i1"),
            Some(WebviewAsset::Inline(b"<h1>hi</h1>".to_vec()))
        );
        assert!(reg.get("missing").is_none());
        reg.remove("i1");
        assert!(reg.get("i1").is_none());
    }

    /// Asserts that the registry keeps answering after a thread panicked
    /// while holding its lock.
    ///
    /// Case: a CEF scheme thread panics mid-request, and the next page load
    /// still needs its assets.
    #[test]
    fn a_poisoned_registry_still_answers() {
        let reg = WebviewAssetRegistry::default();
        reg.insert_inline("i1", b"x".to_vec());
        let poisoner = reg.clone();
        let _ = thread::spawn(move || {
            let _guard = poisoner.0.write().unwrap();
            panic!("poison the registry lock");
        })
        .join();
        assert_eq!(reg.get("i1"), Some(WebviewAsset::Inline(b"x".to_vec())));
    }

    /// Asserts that an inline registration resolves to its bytes at the
    /// index.
    ///
    /// Case: a program registers an inline document and CEF loads it.
    #[test]
    fn an_inline_registration_resolves_to_its_bytes() {
        let reg = WebviewAssetRegistry::default();
        reg.insert_inline("i1", b"<h1>hi</h1>".to_vec());
        match ResolvedOrzma::resolve(&reg, "orzma://i1/index.html") {
            Ok(ResolvedOrzma::Inline(html)) => assert_eq!(html, b"<h1>hi</h1>"),
            _ => panic!("expected the inline document"),
        }
    }

    /// Asserts that an inline registration answers only its index, and
    /// refuses any subresource path.
    ///
    /// Case: an inline document references a script and an image by
    /// relative path.
    #[test]
    fn an_inline_registration_refuses_subresource_paths() {
        let reg = WebviewAssetRegistry::default();
        reg.insert_inline("i1", b"<h1>hi</h1>".to_vec());
        assert!(ResolvedOrzma::resolve(&reg, "orzma://i1/").is_ok());
        assert!(ResolvedOrzma::resolve(&reg, "orzma://i1/index.html").is_ok());
        assert!(matches!(
            ResolvedOrzma::resolve(&reg, "orzma://i1/app.js"),
            Err(WebviewError::UnknownAsset)
        ));
        assert!(matches!(
            ResolvedOrzma::resolve(&reg, "orzma://i1/logo.png"),
            Err(WebviewError::UnknownAsset)
        ));
    }

    /// Asserts that a registered directory resolves to its root and the
    /// requested path, while an unknown handle is refused.
    ///
    /// Case: a page of a live registration loads a script after a page of a
    /// released one tried to.
    #[test]
    fn a_registered_dir_resolves_and_an_unknown_handle_is_refused() {
        let reg = WebviewAssetRegistry::default();
        assert!(matches!(
            ResolvedOrzma::resolve(&reg, "orzma://ghost/index.html"),
            Err(WebviewError::UnknownAsset)
        ));
        reg.insert_dir("h1", PathBuf::from("/abs/ui"));
        match ResolvedOrzma::resolve(&reg, "orzma://h1/app.js") {
            Ok(ResolvedOrzma::Dir { root, path }) => {
                assert_eq!(root, PathBuf::from("/abs/ui"));
                assert_eq!(path, "app.js");
            }
            _ => panic!("expected the directory"),
        }
    }

    /// Asserts that a removed handle no longer resolves.
    ///
    /// Case: a program unregisters its view while a page of it reloads.
    #[test]
    fn a_removed_handle_no_longer_resolves() {
        let reg = WebviewAssetRegistry::default();
        reg.insert_dir("h1", PathBuf::from("/abs/ui"));
        reg.remove("h1");
        assert!(matches!(
            ResolvedOrzma::resolve(&reg, "orzma://h1/index.html"),
            Err(WebviewError::UnknownAsset)
        ));
    }

    /// Asserts that the MIME type handed to CEF drops its parameters.
    ///
    /// Case: a file is served with a charset parameter on its type.
    #[test]
    fn bare_mime_strips_charset_parameter() {
        assert_eq!(bare_mime("text/html; charset=utf-8"), "text/html");
        assert_eq!(
            bare_mime("text/javascript; charset=utf-8"),
            "text/javascript"
        );
        assert_eq!(bare_mime("application/wasm"), "application/wasm");
    }

    /// Asserts that an empty or blank MIME type becomes
    /// `application/octet-stream`.
    ///
    /// Case: a file's type resolves to nothing but parameters.
    #[test]
    fn bare_mime_floors_empty_to_octet_stream() {
        assert_eq!(bare_mime(""), "application/octet-stream");
        assert_eq!(bare_mime("   "), "application/octet-stream");
        assert_eq!(bare_mime("; charset=utf-8"), "application/octet-stream");
    }

    /// Asserts that a forbidden path answers 403 and an oversized file 413,
    /// while a missing asset answers 404.
    ///
    /// Case: a page requests a traversal path, a huge file, and a file that
    /// does not exist.
    #[test]
    fn asset_failures_answer_their_http_status() {
        assert_eq!(
            CefSchemeResponse::from(WebviewError::AssetForbidden).status,
            403
        );
        assert_eq!(
            CefSchemeResponse::from(WebviewError::AssetTooLarge).status,
            413
        );
        assert_eq!(
            CefSchemeResponse::from(WebviewError::AssetNotFound).status,
            404
        );
        assert_eq!(
            CefSchemeResponse::from(WebviewError::UnknownAsset).status,
            404
        );
    }
}
