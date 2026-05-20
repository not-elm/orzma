//! Custom scheme registration for the embedded CEF browser. Shared by
//! `BrowserApp` (browser process) and `HelperApp` (subprocess) so that the
//! `ozmux-ext://` scheme is declared identically in every process — CEF
//! requires `on_register_custom_schemes` to return the same set of schemes
//! across the browser, renderer, GPU, network, and other utility processes.

use cef::{ImplSchemeRegistrar, SchemeOptions, SchemeRegistrar};

/// Registers the `ozmux-ext` custom scheme with CEF using the flags required
/// for ozmux extension front-ends.
///
/// Flags applied (bitwise OR of `CEF_SCHEME_OPTION_*`):
///
/// - `STANDARD` — URL-form parsing for host/path components so
///   `ozmux-ext://<ext>/<path>` resolves like a normal URL.
/// - `SECURE` — treated as an https-origin for mixed-content rules, so a
///   `secure` page can load `ozmux-ext://` sub-resources without being
///   downgraded.
/// - `CORS_ENABLED` + `FETCH_ENABLED` — `XHR` / `fetch` from `ozmux-ext://`
///   pages obey CORS headers returned by the scheme handler.
/// - `DISPLAY_ISOLATED` — forbid navigation/embedding from other origins
///   (Browser Activity → `ozmux-ext://` is denied at the scheme level, on
///   top of any policy enforced by the navigation handler).
pub fn register_ozmux_ext(registrar: &mut SchemeRegistrar) {
    let scheme = cef::CefString::from("ozmux-ext");
    let options = (SchemeOptions::STANDARD.get_raw()
        | SchemeOptions::SECURE.get_raw()
        | SchemeOptions::CORS_ENABLED.get_raw()
        | SchemeOptions::FETCH_ENABLED.get_raw()
        | SchemeOptions::DISPLAY_ISOLATED.get_raw()) as std::os::raw::c_int;
    let ok = registrar.add_custom_scheme(Some(&scheme), options);
    if ok == 0 {
        // NOTE: silent partial registration corrupts URL parsing, CORS, and
        // the upcoming scheme handler dispatch across processes; this MUST
        // surface as an error so operators see the divergence in red logs.
        tracing::error!(
            "cef: failed to register custom scheme `ozmux-ext` — URLs will not parse as standard form, CORS and scheme handler factory will not work in this process"
        );
    }
}
