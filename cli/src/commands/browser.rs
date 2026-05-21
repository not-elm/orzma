//! `ozmux browser` subcommand: opens an embedded Browser Activity in the
//! current terminal pane via the daemon's REST API.

use crate::commands::CommandExecute;
use crate::daemon_client;
use anyhow::{Context, Result};
use clap::Args;

/// `ozmux browser [QUERY...]` — open a Browser Activity in the active pane.
#[derive(Args)]
pub struct Browser {
    /// URL or search query. A single token that looks like a URL
    /// (`example.com`, `https://...`, `localhost:3000`) is opened
    /// directly; anything else is sent to the configured search engine
    /// (`[browser].search_template`, default DuckDuckGo). Mirrors the
    /// toolbar's omnibox.
    #[arg(trailing_var_arg = true)]
    pub query: Vec<String>,
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
    let url = resolve_initial_url(&args.query).await;
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

/// Joins the positional `query` arguments and resolves them through the
/// omnibox classifier (the same algorithm the toolbar URL bar uses).
/// Returns `None` for empty input. Falls back to the default browser
/// config when the user's config file is missing or fails to load — the
/// CLI should still open a browser when only the daemon is reachable.
async fn resolve_initial_url(query: &[String]) -> Option<String> {
    let joined = query.join(" ");
    if joined.trim().is_empty() {
        return None;
    }
    let cfg = ozmux_configs::OzmuxConfigs::load()
        .await
        .unwrap_or_default();
    let resolved =
        ozmux_configs::browser::resolve_omnibox_input(&joined, &cfg.browser.search_template);
    (!resolved.is_empty()).then_some(resolved)
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

    #[tokio::test]
    async fn resolve_initial_url_returns_none_for_empty_args() {
        assert_eq!(resolve_initial_url(&[]).await, None);
    }

    #[tokio::test]
    async fn resolve_initial_url_returns_none_for_whitespace_only() {
        assert_eq!(resolve_initial_url(&["   ".into()]).await, None);
    }

    #[tokio::test]
    async fn resolve_initial_url_prepends_https_for_bare_host() {
        assert_eq!(
            resolve_initial_url(&["example.com".into()]).await,
            Some("https://example.com".to_string())
        );
    }

    #[tokio::test]
    async fn resolve_initial_url_joins_multi_word_query_as_search() {
        let url = resolve_initial_url(&["rust".into(), "async".into(), "tutorial".into()])
            .await
            .expect("resolved URL");
        assert!(url.starts_with("https://duckduckgo.com/?q="));
        assert!(url.contains("rust"));
        assert!(url.contains("async"));
    }

    #[tokio::test]
    async fn resolve_initial_url_passes_through_full_url() {
        assert_eq!(
            resolve_initial_url(&["https://example.com/path".into()]).await,
            Some("https://example.com/path".to_string())
        );
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
