//! CEF integration for extension activities: the `ozmux-ext://` asset scheme and
//! the `window.ozmux` JS extension. (Webview spawn = Task 8; bridge = Task 9.)

use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::host::ExtensionEndpoints;
use ozmux_extension_host::scheme::custom_scheme;

/// The shared asset endpoint the `ozmux-ext://` scheme reads (set on memo-ready
/// in Task 9). Empty until then, so the scheme returns 503.
#[derive(Resource, Clone, Default)]
pub struct AssetEndpoint(pub ExtensionEndpoints);

/// JS defining `window.ozmux` over `cef.emit` / `cef.listen`, injected as a
/// global CEF extension. Mirrors `sdk/typescript/src/cef/ozmux-bridge.ts`.
pub const OZMUX_EXTENSION_JS: &str = include_str!("extension_render/ozmux.js");

/// Builds the `CefPlugin` with the `ozmux-ext://` scheme + `window.ozmux`
/// extension, bound to the given asset endpoint handle.
pub fn cef_plugin(endpoint: &AssetEndpoint) -> CefPlugin {
    CefPlugin {
        // NOTE: "memo" is the extension-NAME segment matched inside
        // ozmux-ext://<name>/…, not the URL scheme name — the scheme is always
        // "ozmux-ext" (scheme::SCHEME_NAME). Frames for other extension names 404.
        custom_schemes: vec![custom_scheme("memo", endpoint.0.clone())],
        extensions: CefExtensions::new().add("ozmux", OZMUX_EXTENSION_JS),
        ..Default::default()
    }
}
