//! `ozmux-ext://<name>/<path>` custom-scheme handler bridging a webview URL to
//! the extension's bytes via `crate::fetch`. Behind the `cef` feature.

#[cfg(feature = "cef")]
use crate::host::{EndpointRegistry, ExtensionEndpoints, FetchError, fetch};
#[cfg(feature = "cef")]
use bevy_cef_core::prelude::{
    CefCustomScheme, CefSchemeBody, CefSchemeHandler, CefSchemeOptions, CefSchemeRequest,
    CefSchemeResponse,
};
#[cfg(feature = "cef")]
use std::sync::Arc;

/// Parses `ozmux-ext://<name>/<path>[?query]` into `(name, path)`; strips the
/// query and defaults an empty path to `"index.html"`. Returns `None` if the
/// URL is not a well-formed `ozmux-ext://` URL with a non-empty `<name>`.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn parse_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("ozmux-ext://")?;
    let rest = rest
        .split_once(['?', '#'])
        .map_or(rest, |(before, _)| before);
    let (name, path) = match rest.split_once('/') {
        Some((n, p)) => (n, p),
        None => (rest, ""),
    };
    if name.is_empty() {
        return None;
    }
    let path = if path.is_empty() { "index.html" } else { path };
    Some((name, path))
}

/// Returns the bare media type (drops any `;`-delimited parameters) for CEF's
/// `mime_type` field, flooring an empty/blank input to `application/octet-stream`.
/// CEF expects a bare type (e.g. `text/html`); a full `Content-Type` value with
/// parameters (`text/html; charset=utf-8`) is not recognized, so Chromium fails
/// to classify the document and renders blank. An empty `mime_type` triggers the
/// same blank render, so it is floored to `application/octet-stream` (matching
/// the SDK file handler's default) rather than passed through empty.
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn bare_mime(content_type: &str) -> String {
    let bare = content_type.split(';').next().unwrap_or("").trim();
    if bare.is_empty() {
        "application/octet-stream".to_string()
    } else {
        bare.to_string()
    }
}

/// The custom scheme name registered with CEF.
#[cfg(feature = "cef")]
pub const SCHEME_NAME: &str = "ozmux-ext";

/// Serves `ozmux-ext://<name>/<path>` for every registered extension by
/// dispatching on `<name>` through a shared endpoint registry and fetching from
/// the matched extension's live socket endpoint.
#[cfg(feature = "cef")]
pub struct OzmuxExtScheme {
    registry: EndpointRegistry,
}

#[cfg(feature = "cef")]
impl OzmuxExtScheme {
    /// Builds a handler bound to the shared endpoint registry.
    pub fn new(registry: EndpointRegistry) -> Self {
        Self { registry }
    }
}

/// Resolves the extension endpoint for an `ozmux-ext://<name>/<path>` URL via the
/// registry. Returns `Ok((endpoints, path))` to fetch, or `Err(status)` for a
/// direct error response (404 unknown/unparseable name).
#[cfg(feature = "cef")]
fn resolve_request<'a>(
    registry: &EndpointRegistry,
    url: &'a str,
) -> Result<(ExtensionEndpoints, &'a str), u16> {
    let (name, path) = parse_url(url).ok_or(404u16)?;
    let endpoints = registry.get(name).ok_or(404u16)?;
    Ok((endpoints, path))
}

#[cfg(feature = "cef")]
impl CefSchemeHandler for OzmuxExtScheme {
    fn handle(&self, request: &CefSchemeRequest) -> CefSchemeResponse {
        let (endpoints, path) = match resolve_request(&self.registry, &request.url) {
            Ok(resolved) => resolved,
            Err(404) => return CefSchemeResponse::not_found(),
            Err(status) => return status_text(status, "extension dispatch failed"),
        };
        match fetch(&endpoints, path) {
            Ok(r) => {
                let mime = bare_mime(&r.content_type);
                bevy::log::debug!(
                    url = %request.url,
                    status = r.status,
                    mime = %mime,
                    bytes = r.body.len(),
                    "ozmux-ext asset served"
                );
                CefSchemeResponse {
                    status: r.status,
                    mime_type: mime,
                    headers: Vec::new(),
                    body: CefSchemeBody::Bytes(r.body),
                }
            }
            Err(FetchError::NotReady) => {
                bevy::log::debug!(url = %request.url, "ozmux-ext asset endpoint not ready");
                status_text(503, "extension not ready")
            }
            Err(e) => {
                bevy::log::warn!(url = %request.url, error = %e, "ozmux-ext asset fetch failed");
                status_text(502, "extension fetch failed")
            }
        }
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

/// Builds the `ozmux-ext` scheme registration to pass to `CefPlugin`, dispatching
/// every `ozmux-ext://<name>/…` URL through the shared endpoint registry.
#[cfg(feature = "cef")]
pub fn custom_scheme(registry: EndpointRegistry) -> CefCustomScheme {
    CefCustomScheme {
        name: SCHEME_NAME.to_string(),
        options: CefSchemeOptions::STANDARD
            | CefSchemeOptions::SECURE
            | CefSchemeOptions::CORS_ENABLED
            | CefSchemeOptions::FETCH_ENABLED
            | CefSchemeOptions::DISPLAY_ISOLATED,
        domain: None,
        handler: Arc::new(OzmuxExtScheme::new(registry)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_path() {
        assert_eq!(
            parse_url("ozmux-ext://hello/index.html"),
            Some(("hello", "index.html"))
        );
        assert_eq!(
            parse_url("ozmux-ext://hello/sub/a.css"),
            Some(("hello", "sub/a.css"))
        );
    }

    #[test]
    fn empty_path_defaults_to_index() {
        assert_eq!(
            parse_url("ozmux-ext://hello/"),
            Some(("hello", "index.html"))
        );
        assert_eq!(
            parse_url("ozmux-ext://hello"),
            Some(("hello", "index.html"))
        );
    }

    #[test]
    fn strips_query_and_fragment() {
        assert_eq!(
            parse_url("ozmux-ext://hello/a.js?v=2"),
            Some(("hello", "a.js"))
        );
        assert_eq!(
            parse_url("ozmux-ext://hello/a.js#anchor"),
            Some(("hello", "a.js"))
        );
    }

    #[test]
    fn rejects_foreign_or_empty() {
        assert_eq!(parse_url("https://hello/x"), None);
        assert_eq!(parse_url("ozmux-ext:///x"), None);
    }

    #[test]
    fn bare_mime_strips_charset_parameter() {
        assert_eq!(bare_mime("text/html; charset=utf-8"), "text/html");
        assert_eq!(
            bare_mime("text/javascript; charset=utf-8"),
            "text/javascript"
        );
        assert_eq!(bare_mime("application/wasm"), "application/wasm");
    }

    #[test]
    fn bare_mime_floors_empty_to_octet_stream() {
        assert_eq!(bare_mime(""), "application/octet-stream");
        assert_eq!(bare_mime("   "), "application/octet-stream");
        assert_eq!(bare_mime("; charset=utf-8"), "application/octet-stream");
    }

    #[cfg(feature = "cef")]
    #[test]
    fn dispatch_resolves_registered_name_and_404s_unknown_even_after_late_insert() {
        use crate::host::{EndpointRegistry, ExtensionEndpoints};
        let registry = EndpointRegistry::default();
        // unknown name → 404
        assert_eq!(
            resolve_request(&registry, "ozmux-ext://ghost/index.html").err(),
            Some(404)
        );
        // register AFTER construction → now resolvable (handler reads live registry)
        registry.insert("memo", ExtensionEndpoints::default());
        let (_ep, path) =
            resolve_request(&registry, "ozmux-ext://memo/app.js").expect("registered");
        assert_eq!(path, "app.js");
        // empty path defaults to index.html (parse_url behavior preserved)
        let (_ep, path2) = resolve_request(&registry, "ozmux-ext://memo").expect("registered");
        assert_eq!(path2, "index.html");
    }
}
