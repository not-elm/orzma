//! Cmd/Ctrl-click hyperlink activation helpers for the Ozma terminal: the
//! platform link-activation modifier test and the allowlist-gated URI opener,
//! invoked from the mouse dispatcher (`crate::mouse`). Hover-cursor feedback and
//! the renderer underline (`HyperlinkHoverState`) are owned by the host's
//! hyperlink hover system, not here.

use bevy::prelude::*;
use ozma_tty_engine::ProtocolModifiers;
use ozma_tty_renderer::schema::is_allowed;

/// Returns `true` when the platform link-activation modifier is held: Cmd on
/// macOS, Ctrl elsewhere.
pub(crate) fn link_modifier_held(mods: &ProtocolModifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.meta
    } else {
        mods.ctrl
    }
}

/// Validates `uri` against the shared allowlist and opens it via the OS default
/// handler. Disallowed URIs are dropped with a debug log.
pub(crate) fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        warn!("hyperlink: failed to open {}: {}", uri, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_modifier_matches_platform() {
        let mut m = ProtocolModifiers::default();
        assert!(!link_modifier_held(&m));
        if cfg!(target_os = "macos") {
            m.meta = true;
        } else {
            m.ctrl = true;
        }
        assert!(link_modifier_held(&m));
    }
}
