//! Detached spawn of the `ozmux-client` Tauri launcher with a deep-link
//! URL pointing at a specific session.

use anyhow::{Context, Result};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::process::{Command, Stdio};

/// Build the deep-link URL for the given session id.
pub(super) fn deep_link_url(session_id: &str) -> String {
    let encoded = utf8_percent_encode(session_id, NON_ALPHANUMERIC);
    format!("{}/?session={}", daemon_bootstrap::HTTP_BASE_URL, encoded)
}

/// Spawn `ozmux-client` (or whichever binary `OZMUX_CLIENT_BIN` points at)
/// detached from the CLI's controlling tty, passing the session-scoped URL
/// as its first positional argument. Returns an error only when the
/// `spawn()` syscall itself fails (e.g. ENOENT); the child's exit code is
/// not awaited.
pub(super) fn spawn_detached(session_id: &str) -> Result<()> {
    let bin = std::env::var("OZMUX_CLIENT_BIN").unwrap_or_else(|_| "ozmux-client".into());
    let url = deep_link_url(session_id);

    let mut cmd = Command::new(&bin);
    cmd.arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::detach::configure_detached(&mut cmd);

    cmd.spawn().with_context(|| format!("spawn {bin} {url}"))?;
    // NOTE: drop the child handle without waiting; the launcher is
    // intentionally detached.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_url_encodes_session_id() {
        let url = deep_link_url("abc-123");
        assert!(url.starts_with("http://"));
        assert!(url.contains("?session=abc%2D123"));
    }

    #[test]
    fn deep_link_url_escapes_dangerous_chars() {
        let url = deep_link_url("hi&id=evil");
        assert!(url.contains("?session=hi%26id%3Devil"));
    }
}
