//! Preload-script builder for orzma-managed webviews: the `window.orzma` Tier 1
//! back-channel bridge (`orzma_bridge.js`), injected ahead of any
//! registering-program preload scripts.

use bevy_cef::prelude::PreloadScripts;

/// JS defining the unified `window.orzma` back-channel bridge (`.call` / `.on`),
/// injected per Tier 1 bridged webview as a `PreloadScripts` entry. Frozen onto
/// `window` so a page cannot shadow it.
pub(super) const ORZMA_BRIDGE_JS: &str = include_str!("orzma_bridge.js");

/// Builds the preload for a bridged Tier 1 webview: the `window.orzma`
/// back-channel bridge, followed by the registering program's user scripts.
/// No capability grant — the bridge routes to the registering program, not the
/// host.
pub(crate) fn build_preload(user: &[String]) -> PreloadScripts {
    let mut scripts = vec![ORZMA_BRIDGE_JS.to_string()];
    scripts.extend(user.iter().cloned());
    PreloadScripts::from(scripts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_preload_injects_the_orzma_bridge_first() {
        let preload = build_preload(&[]);
        assert_eq!(preload.0.len(), 1, "orzma bridge only");
        assert_eq!(preload.0[0], ORZMA_BRIDGE_JS);
        assert!(
            ORZMA_BRIDGE_JS.contains("window, 'orzma'")
                && ORZMA_BRIDGE_JS.contains("defineProperty")
        );
        assert!(ORZMA_BRIDGE_JS.contains("kind: 'orzma.call'"));
        assert!(ORZMA_BRIDGE_JS.contains("kind: 'orzma.emit'"));
    }

    #[test]
    fn build_preload_carries_no_hint_engine() {
        let preload = build_preload(&[]);
        assert!(
            !preload.0.iter().any(|s| s.contains("hints:show")),
            "the host no longer injects the link-hint engine"
        );
    }

    #[test]
    fn user_scripts_are_appended_after_the_bridge() {
        let user = vec!["window.USER = 1;".to_string()];
        let preload = build_preload(&user);
        assert_eq!(preload.0.len(), 2);
        assert_eq!(preload.0[0], ORZMA_BRIDGE_JS, "bridge first");
        assert_eq!(preload.0[1], "window.USER = 1;", "user script last");
    }
}
