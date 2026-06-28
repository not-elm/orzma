//! Allowlist-gated hyperlink URI opener for the Ozma terminal, invoked by the
//! mouse apply observer (`crate::mouse`). The platform link-activation modifier
//! test and hover-cursor feedback / renderer underline (`HyperlinkHoverState`)
//! are owned by the host's hyperlink hover system, not here.

use bevy::prelude::*;
use ozma_tty_renderer::schema::is_allowed;

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
