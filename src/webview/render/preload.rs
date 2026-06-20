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
/// back-channel bridge. No capability grant — the bridge routes to the
/// registering program, not the host.
pub(crate) fn build_dynamic_preload() -> PreloadScripts {
    PreloadScripts::from([OZMA_BRIDGE_JS.to_string()])
}

/// Builds the preload for a bridged URL webview: the `window.ozma` bridge
/// followed by the link-hint engine. Order matters — the hint engine consumes
/// `window.ozma`, which the bridge defines, so the bridge entry is first.
pub(crate) fn build_url_preload() -> PreloadScripts {
    PreloadScripts::from([OZMA_BRIDGE_JS.to_string(), OZMA_HINTS_JS.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_preload_injects_only_the_ozma_bridge() {
        let preload = build_dynamic_preload();
        assert_eq!(preload.0.len(), 1, "ozma bridge only");
        assert_eq!(preload.0[0], OZMA_BRIDGE_JS);
        assert!(
            OZMA_BRIDGE_JS.contains("window, 'ozma'") && OZMA_BRIDGE_JS.contains("defineProperty")
        );
        assert!(OZMA_BRIDGE_JS.contains("kind: 'ozma.call'"));
        assert!(OZMA_BRIDGE_JS.contains("kind: 'ozma.emit'"));
    }

    #[test]
    fn url_preload_injects_bridge_then_hints_in_order() {
        let preload = build_url_preload();
        assert_eq!(preload.0.len(), 2, "bridge + hints");
        assert_eq!(preload.0[0], OZMA_BRIDGE_JS, "bridge must run first");
        assert_eq!(preload.0[1], OZMA_HINTS_JS, "hints run after the bridge");
        assert!(OZMA_HINTS_JS.contains("hints:show"));
        assert!(OZMA_HINTS_JS.contains("hintResult"));
    }

    #[test]
    fn dynamic_preload_has_no_hints() {
        let preload = build_dynamic_preload();
        assert!(
            !preload.0.iter().any(|s| s.contains("hints:show")),
            "inline/dir webviews must not carry the hint engine"
        );
    }
}
