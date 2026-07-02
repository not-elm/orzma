//! Hyperlink open action: opens an allowlist-validated URI via the OS default
//! handler, gated on the target terminal still existing.

use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_renderer::schema::is_allowed;

/// Opens `uri` in the host browser / handler, gated on the target terminal
/// still existing.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalOpenUri {
    /// The terminal entity the link belongs to; the open is suppressed if it
    /// no longer exists.
    #[event_target]
    pub entity: Entity,
    /// The URI to open.
    pub uri: String,
}

/// Registers the open-uri apply observer.
pub(super) struct OpenUriPlugin;

impl Plugin for OpenUriPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_open_uri);
    }
}

/// Applies a `TerminalOpenUri`: opens the link in the host handler, but only
/// while the target terminal still exists — parity with the legacy apply
/// path, which gated every effect behind the target's presence.
fn on_terminal_open_uri(ev: On<TerminalOpenUri>, terminals: Query<(), With<OzmaTerminal>>) {
    if terminals.get(ev.entity).is_ok() {
        try_open_uri(&ev.uri);
    }
}

/// Validates `uri` against the shared allowlist and opens it via the OS default
/// handler. Disallowed URIs are dropped with a debug log.
fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        warn!("hyperlink: failed to open {}: {}", uri, e);
    }
}
