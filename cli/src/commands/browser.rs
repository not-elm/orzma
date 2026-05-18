//! `ozmux browser` subcommand: opens an embedded Browser Activity in the
//! current terminal pane via the daemon's REST API.

use crate::commands::CommandExecute;
use crate::daemon_client;
use anyhow::{Context, Result};
use clap::Args;

/// `ozmux browser [URL]` — open a Browser Activity in the active pane.
#[derive(Args)]
pub struct Browser {
    /// URL to open. Schemes are added automatically: `foo.com` → `https://foo.com`.
    pub url: Option<String>,
    /// Named storage profile to use. Defaults to `default`.
    #[arg(long, conflicts_with = "incognito")]
    pub profile: Option<String>,
    /// Open in an ephemeral in-memory profile (no disk persistence).
    #[arg(long)]
    pub incognito: bool,
    /// Split the current pane and open the Browser Activity in the new pane
    /// instead of seating it inside the current pane.
    #[arg(long, short = 's', value_enum)]
    pub split: Option<SplitDirection>,
}

/// Direction the new pane is placed relative to the current pane when
/// `ozmux browser --split <DIR>` is used. Wire-mapped to CEF's
/// `(orientation, side)` pair: `right`/`left` → horizontal split,
/// `down`/`up` → vertical split; `right`/`down` → `after`, `left`/`up` → `before`.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum SplitDirection {
    /// New pane appears to the right (horizontal split, `side = after`).
    Right,
    /// New pane appears to the left (horizontal split, `side = before`).
    Left,
    /// New pane appears below (vertical split, `side = after`).
    Down,
    /// New pane appears above (vertical split, `side = before`).
    Up,
}

/// Maps a user-facing `SplitDirection` to the `(orientation, side)` pair the
/// daemon's `POST .../split` endpoint expects. Strings match the serde
/// representation of `ozmux_multiplexer::{SplitOrientation, Side}`
/// (lowercase).
fn split_direction_to_wire(d: SplitDirection) -> (&'static str, &'static str) {
    match d {
        SplitDirection::Right => ("horizontal", "after"),
        SplitDirection::Left => ("horizontal", "before"),
        SplitDirection::Down => ("vertical", "after"),
        SplitDirection::Up => ("vertical", "before"),
    }
}

impl CommandExecute for Browser {
    async fn run(self) -> Result<()> {
        run(self).await
    }
}

/// Run the `ozmux browser` subcommand. Reads the current pane/window from
/// PTY-injected env vars, then either splits the pane (when `--split` is
/// given) or creates the activity in the current pane and activates it.
pub async fn run(args: Browser) -> Result<()> {
    let wid = std::env::var("OZMUX_WINDOW_ID")
        .context("OZMUX_WINDOW_ID not set (are you running inside an ozmux pane?)")?;
    let pid = std::env::var("OZMUX_PANE_ID")
        .context("OZMUX_PANE_ID not set (are you running inside an ozmux pane?)")?;
    let url = args.url.as_deref().map(normalize_url);
    let profile = resolve_profile(args.profile.as_deref(), args.incognito);

    match args.split {
        Some(direction) => {
            let (orientation, side) = split_direction_to_wire(direction);
            daemon_client::split_browser_activity(
                &wid,
                &pid,
                orientation,
                side,
                url.as_deref(),
                profile,
            )
            .await
        }
        None => {
            let aid =
                daemon_client::create_browser_activity(&wid, &pid, url.as_deref(), profile).await?;
            daemon_client::activate(&wid, &pid, &aid).await
        }
    }
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

    #[test]
    fn split_direction_to_wire_right_maps_to_horizontal_after() {
        assert_eq!(
            split_direction_to_wire(SplitDirection::Right),
            ("horizontal", "after")
        );
    }

    #[test]
    fn split_direction_to_wire_left_maps_to_horizontal_before() {
        assert_eq!(
            split_direction_to_wire(SplitDirection::Left),
            ("horizontal", "before")
        );
    }

    #[test]
    fn split_direction_to_wire_down_maps_to_vertical_after() {
        assert_eq!(
            split_direction_to_wire(SplitDirection::Down),
            ("vertical", "after")
        );
    }

    #[test]
    fn split_direction_to_wire_up_maps_to_vertical_before() {
        assert_eq!(
            split_direction_to_wire(SplitDirection::Up),
            ("vertical", "before")
        );
    }
}
