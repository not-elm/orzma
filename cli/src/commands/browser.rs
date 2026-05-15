//! `ozmux browser` subcommand: opens an embedded Browser Activity in the
//! current terminal pane via the daemon's REST API.

use crate::cli::Browser;
use crate::daemon_client;
use anyhow::{Context, Result};

/// Run the `ozmux browser` subcommand. Reads the current pane/window from
/// PTY-injected env vars, asks the daemon to create the activity, then
/// activates it.
pub async fn run(args: Browser) -> Result<()> {
    let wid = std::env::var("OZMUX_WINDOW_ID")
        .context("OZMUX_WINDOW_ID not set (are you running inside an ozmux pane?)")?;
    let pid = std::env::var("OZMUX_PANE_ID")
        .context("OZMUX_PANE_ID not set (are you running inside an ozmux pane?)")?;
    let url = args.url.as_deref().map(normalize_url);
    let aid = daemon_client::create_browser_activity(&wid, &pid, url.as_deref()).await?;
    daemon_client::activate(&wid, &pid, &aid).await?;
    Ok(())
}

/// Add a default scheme to a URL-like input. Bare hosts get `https://`;
/// inputs that already have a scheme (including `about:`) pass through.
fn normalize_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("about:")
    {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_https_passes_through() {
        assert_eq!(normalize_url("https://x.com"), "https://x.com");
    }

    #[test]
    fn already_http_passes_through() {
        assert_eq!(normalize_url("http://x.com"), "http://x.com");
    }

    #[test]
    fn bare_host_gets_https() {
        assert_eq!(normalize_url("x.com"), "https://x.com");
    }

    #[test]
    fn about_blank_passes_through() {
        assert_eq!(normalize_url("about:blank"), "about:blank");
    }

    #[test]
    fn path_only_gets_https() {
        assert_eq!(
            normalize_url("example.com/path"),
            "https://example.com/path"
        );
    }
}
