//! Preload-script builder for ozmux-managed webviews: the `window.ozma` Tier 1
//! back-channel bridge (`ozma_bridge.js`).

use bevy_cef::prelude::PreloadScripts;

/// JS defining the unified `window.ozma` back-channel bridge (`.call` / `.on`),
/// injected per Tier 1 dynamic webview as a `PreloadScripts` entry. Frozen onto
/// `window` so a page cannot shadow it.
pub(super) const OZMA_BRIDGE_JS: &str = include_str!("ozma_bridge.js");

/// Builds the preload for a Tier 1 dynamic webview: the `window.ozma`
/// back-channel bridge. No capability grant — the bridge routes to the
/// registering program, not the host.
pub(crate) fn build_dynamic_preload() -> PreloadScripts {
    PreloadScripts::from([OZMA_BRIDGE_JS.to_string()])
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
    }
}
