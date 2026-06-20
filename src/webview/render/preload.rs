//! Preload-script builders for ozmux-managed webviews: the `window.ozma` Tier 1
//! back-channel bridge (`ozma_bridge.js`), and — for URL webviews — the
//! Vimium-style link-hint engine (`ozma_hints.js`) layered after it.

use bevy_cef::prelude::PreloadScripts;

/// JS defining the unified `window.ozma` back-channel bridge (`.call` / `.on`),
/// injected per Tier 1 dynamic webview as a `PreloadScripts` entry. Frozen onto
/// `window` so a page cannot shadow it.
pub(super) const OZMA_BRIDGE_JS: &str = include_str!("ozma_bridge.js");

/// JS implementing the Vimium-style link-hint engine (`hints:show` / `hints:key`
/// / `hints:hide` handlers, reporting `hintResult`). Injected after the bridge
/// for URL webviews, which it depends on for `window.ozma`.
const OZMA_HINTS_JS: &str = include_str!("ozma_hints.js");

/// Builds the preload for a Tier 1 dynamic webview: the `window.ozma`
/// back-channel bridge, followed by the registering program's user scripts.
/// No capability grant — the bridge routes to the registering program, not the
/// host.
pub(crate) fn build_dynamic_preload(user: &[String]) -> PreloadScripts {
    let mut scripts = vec![OZMA_BRIDGE_JS.to_string()];
    scripts.extend(user.iter().cloned());
    PreloadScripts::from(scripts)
}

/// Builds the preload for a bridged URL webview: the `window.ozma` bridge, then
/// the link-hint engine, then the registering program's user scripts. Order
/// matters — the hint engine consumes `window.ozma` (defined by the bridge), and
/// user scripts run last so they may use both.
pub(crate) fn build_url_preload(user: &[String]) -> PreloadScripts {
    let mut scripts = vec![OZMA_BRIDGE_JS.to_string(), OZMA_HINTS_JS.to_string()];
    scripts.extend(user.iter().cloned());
    PreloadScripts::from(scripts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_preload_injects_only_the_ozma_bridge() {
        let preload = build_dynamic_preload(&[]);
        assert_eq!(preload.0.len(), 1, "ozma bridge only");
        assert_eq!(preload.0[0], OZMA_BRIDGE_JS);
        assert!(
            OZMA_BRIDGE_JS.contains("window, 'ozma'") && OZMA_BRIDGE_JS.contains("defineProperty")
        );
        assert!(OZMA_BRIDGE_JS.contains("kind: 'ozma.call'"));
    }

    #[test]
    fn url_preload_injects_bridge_then_hints_in_order() {
        let preload = build_url_preload(&[]);
        assert_eq!(preload.0.len(), 2, "bridge + hints");
        assert_eq!(preload.0[0], OZMA_BRIDGE_JS, "bridge must run first");
        assert_eq!(preload.0[1], OZMA_HINTS_JS, "hints run after the bridge");
        assert!(OZMA_HINTS_JS.contains("hints:show"));
        assert!(OZMA_HINTS_JS.contains("hintResult"));
    }

    #[test]
    fn dynamic_preload_has_no_hints() {
        let preload = build_dynamic_preload(&[]);
        assert!(
            !preload.0.iter().any(|s| s.contains("hints:show")),
            "inline/dir webviews must not carry the hint engine"
        );
    }

    #[test]
    fn user_scripts_are_appended_after_host_scripts() {
        let user = vec!["window.USER = 1;".to_string()];

        let dynamic = build_dynamic_preload(&user);
        assert_eq!(dynamic.0.len(), 2);
        assert_eq!(dynamic.0[0], OZMA_BRIDGE_JS, "bridge first");
        assert_eq!(dynamic.0[1], "window.USER = 1;", "user script last");

        let url = build_url_preload(&user);
        assert_eq!(url.0.len(), 3);
        assert_eq!(url.0[0], OZMA_BRIDGE_JS);
        assert_eq!(url.0[1], OZMA_HINTS_JS);
        assert_eq!(
            url.0[2], "window.USER = 1;",
            "user script after bridge+hints"
        );
    }
}
