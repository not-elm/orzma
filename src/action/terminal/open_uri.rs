//! Hyperlink open action: opens an allowlist-validated URI via the OS default
//! handler, gated on the target terminal still existing.

use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use orzma_vt::prelude::is_allowed;
use std::thread;

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

/// Adds the apply path for `TerminalOpenUri`.
pub(super) struct OpenUriPlugin;

impl Plugin for OpenUriPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_open_uri);
    }
}

/// Applies a `TerminalOpenUri` by opening the link in the host handler while
/// the target terminal still exists.
fn on_terminal_open_uri(ev: On<TerminalOpenUri>, terminals: Query<(), With<OrzmaTerminal>>) {
    if terminals.get(ev.entity).is_ok() {
        try_open_uri(&ev.uri);
    }
}

/// Validates `uri` against the shared allowlist and opens it via the OS default
/// handler on a worker thread, after `escape_for_command_line`. Disallowed URIs
/// are dropped with a debug log, and a failed open is logged as a warning.
fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    // NOTE: the uri comes from program output, and on Windows the shell pastes
    // it into the command line its scheme's handler registers; an unencoded
    // `"` or space there would hand the handler extra arguments.
    let target = escape_for_command_line(uri);
    // NOTE: on Windows `open::that_detached` is a synchronous ShellExecuteExW
    // that lasts until the handler starts or its error dialog is dismissed, so
    // running it on the main thread would stall every frame until then.
    let spawned = thread::Builder::new()
        .name("orzma-open-uri".to_owned())
        .spawn(move || {
            if let Err(e) = open::that_detached(&target) {
                warn!("hyperlink: failed to open {}: {}", target, e);
            }
        });
    if let Err(e) = spawned {
        warn!(
            "hyperlink: cannot start the thread that opens {}: {}",
            uri, e
        );
    }
}

/// `uri` with every ASCII control character, space, and `"` percent-encoded;
/// every other character, `%` included, is kept as written.
fn escape_for_command_line(uri: &str) -> String {
    let mut escaped = String::with_capacity(uri.len());
    for c in uri.chars() {
        if c.is_ascii_control() || c == ' ' || c == '"' {
            escaped.push_str(&format!("%{:02X}", u32::from(c)));
        } else {
            escaped.push(c);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a quote, a space, and a control character in a URI are
    /// percent-encoded while an existing escape and a backslash are kept.
    ///
    /// Case: a program prints an OSC 8 mailto link whose target carries a
    /// quote and spaces meant to add an attachment argument to the mail
    /// handler, next to an already-encoded subject.
    #[test]
    fn quotes_spaces_and_controls_are_percent_encoded() {
        assert_eq!(
            escape_for_command_line("mailto:a@b.example\" /a \"C:\\x?subject=Hi%20there\t"),
            "mailto:a@b.example%22%20/a%20%22C:\\x?subject=Hi%20there%09"
        );
    }

    /// Asserts that a well-formed URI passes through unchanged.
    ///
    /// Case: a build tool prints an OSC 8 link to its documentation page.
    #[test]
    fn a_well_formed_uri_is_kept_as_written() {
        let uri = "https://example.com/docs?q=a%20b&lang=ja#install";
        assert_eq!(escape_for_command_line(uri), uri);
    }
}
