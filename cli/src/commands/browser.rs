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
    let profile = resolve_profile(args.profile.as_deref(), args.incognito);
    let aid = daemon_client::create_browser_activity(&wid, &pid, url.as_deref(), profile).await?;
    daemon_client::activate(&wid, &pid, &aid).await?;
    Ok(())
}

/// Build the JSON `profile` object for the create-activity request.
/// `--incognito` wins over `--profile`; with neither, the `default`
/// named profile is used.
fn resolve_profile(profile: Option<&str>, incognito: bool) -> serde_json::Value {
    if incognito {
        serde_json::json!({ "kind": "incognito" })
    } else {
        let name = profile.unwrap_or("default");
        serde_json::json!({ "kind": "named", "name": name })
    }
}

/// Add a default scheme to a URL-like input. Bare hosts get `https://`;
/// inputs that already carry any scheme (`://` present) or start with
/// `about:` pass through unchanged. This handles `ftp://`, `chrome://`,
/// `file://`, etc. without incorrectly prepending `https://`.
fn normalize_url(input: &str) -> String {
    if input.contains("://") || input.starts_with("about:") {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_profile_defaults_to_named_default() {
        assert_eq!(resolve_profile(None, false), json_named("default"));
    }

    #[test]
    fn resolve_profile_named_when_given() {
        assert_eq!(resolve_profile(Some("work"), false), json_named("work"));
    }

    #[test]
    fn resolve_profile_incognito_overrides() {
        assert_eq!(
            resolve_profile(None, true),
            serde_json::json!({ "kind": "incognito" })
        );
    }

    fn json_named(n: &str) -> serde_json::Value {
        serde_json::json!({ "kind": "named", "name": n })
    }

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

    #[test]
    fn ftp_scheme_passes_through() {
        assert_eq!(normalize_url("ftp://x.com"), "ftp://x.com");
    }

    #[test]
    fn chrome_scheme_passes_through() {
        assert_eq!(normalize_url("chrome://settings"), "chrome://settings");
    }
}
