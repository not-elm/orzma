//! `ozmux-ext://<name>/<path>` custom-scheme handler bridging a webview URL to
//! the extension's bytes via [`crate::fetch`]. Behind the `cef` feature.

#[cfg(feature = "cef")]
use crate::host::{ExtensionEndpoints, FetchError, fetch};
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

/// The custom scheme name registered with CEF.
#[cfg(feature = "cef")]
pub const SCHEME_NAME: &str = "ozmux-ext";

/// Serves `ozmux-ext://<name>/<path>` for one extension by fetching from its
/// live socket endpoint.
#[cfg(feature = "cef")]
pub struct OzmuxExtScheme {
    name: String,
    endpoints: ExtensionEndpoints,
}

#[cfg(feature = "cef")]
impl OzmuxExtScheme {
    /// Builds a handler bound to one extension `name` + its endpoint handle.
    pub fn new(name: impl Into<String>, endpoints: ExtensionEndpoints) -> Self {
        Self {
            name: name.into(),
            endpoints,
        }
    }
}

#[cfg(feature = "cef")]
impl CefSchemeHandler for OzmuxExtScheme {
    fn handle(&self, request: &CefSchemeRequest) -> CefSchemeResponse {
        let Some((name, path)) = parse_url(&request.url) else {
            return CefSchemeResponse::not_found();
        };
        if name != self.name {
            return CefSchemeResponse::not_found();
        }
        match fetch(&self.endpoints, path) {
            Ok(r) => CefSchemeResponse {
                status: r.status,
                mime_type: r.content_type,
                headers: Vec::new(),
                body: CefSchemeBody::Bytes(r.body),
            },
            Err(FetchError::NotReady) => status_text(503, "extension not ready"),
            Err(_) => status_text(502, "extension fetch failed"),
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

/// Builds the `ozmux-ext` scheme registration to pass to `CefPlugin`.
#[cfg(feature = "cef")]
pub fn custom_scheme(name: impl Into<String>, endpoints: ExtensionEndpoints) -> CefCustomScheme {
    CefCustomScheme {
        name: SCHEME_NAME.to_string(),
        options: CefSchemeOptions::STANDARD
            | CefSchemeOptions::SECURE
            | CefSchemeOptions::CORS_ENABLED
            | CefSchemeOptions::FETCH_ENABLED
            | CefSchemeOptions::DISPLAY_ISOLATED,
        domain: None,
        handler: Arc::new(OzmuxExtScheme::new(name, endpoints)),
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
}
