//! Minimal REST client targeting the daemon on `127.0.0.1:3200`.

use anyhow::{Context, Result};
use serde_json::json;
use uuid::Uuid;

const BASE_URL: &str = "http://127.0.0.1:3200";

/// Create a new Browser Activity in the given pane. Returns the
/// freshly-generated `activity_id`. The caller must subsequently activate
/// it (`POST .../activate`) to bring it foreground. The `profile` JSON
/// (named or incognito) is embedded in the activity `kind`.
pub(crate) async fn create_browser_activity(
    wid: &str,
    pid: &str,
    initial_url: Option<&str>,
    profile: serde_json::Value,
) -> Result<String> {
    let aid = Uuid::new_v4().to_string();
    let body = json!({
        "activity": {
            "activity_id": aid,
            "kind": {
                "type": "browser",
                "initial_url": initial_url,
                "profile": profile,
            }
        }
    });
    let resp = reqwest::Client::new()
        .post(format!("{BASE_URL}/windows/{wid}/panes/{pid}/activities"))
        .json(&body)
        .send()
        .await
        .context("POST create-activity")?;
    let status = resp.status();
    anyhow::ensure!(
        status.is_success(),
        "create-activity failed: {} ({})",
        status,
        resp.text().await.unwrap_or_default()
    );
    Ok(aid)
}

/// Activate an existing Activity so it becomes the foreground tab in its pane.
pub(crate) async fn activate(wid: &str, pid: &str, aid: &str) -> Result<()> {
    let resp = reqwest::Client::new()
        .post(format!(
            "{BASE_URL}/windows/{wid}/panes/{pid}/activities/{aid}/activate"
        ))
        .send()
        .await
        .context("POST activate")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "activate failed: {} ({})",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
    Ok(())
}

/// Splits `target_pid` in `wid` and seats a new Browser Activity in the
/// freshly-created pane. `orientation` and `side` must be lowercase strings
/// matching the daemon's `SplitOrientation` (`"horizontal"`/`"vertical"`)
/// and `Side` (`"before"`/`"after"`) serde shapes. The daemon's split
/// handler already marks the new pane active, so no follow-up activate
/// call is needed.
#[expect(dead_code, reason = "Wired by Task 3 when --split flag is added to Browser::run()")]
pub(crate) async fn split_browser_activity(
    wid: &str,
    target_pid: &str,
    orientation: &str,
    side: &str,
    initial_url: Option<&str>,
    profile: serde_json::Value,
) -> Result<()> {
    let aid = Uuid::new_v4().to_string();
    let body = json!({
        "orientation": orientation,
        "side": side,
        "activity": {
            "activity_id": aid,
            "kind": {
                "type": "browser",
                "initial_url": initial_url,
                "profile": profile,
            }
        }
    });
    let resp = reqwest::Client::new()
        .post(format!("{BASE_URL}/windows/{wid}/panes/{target_pid}/split"))
        .json(&body)
        .send()
        .await
        .context("POST split")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "split failed: {} ({})",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
    Ok(())
}
