//! `ozmux session new` — create a session via the daemon's HTTP API.

use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::commands::CommandExecute;
use crate::commands::daemon;

const NEW_TIMEOUT: Duration = Duration::from_secs(5);

/// Arguments for the `session new` subcommand.
#[derive(Args)]
pub(crate) struct NewArgs {
    /// Name for the new session. The daemon assigns a default if omitted.
    #[arg(short = 's', long)]
    name: Option<String>,
    /// Open the new session in the Tauri client window after creating it.
    #[arg(long)]
    open: bool,
}

#[derive(Serialize)]
struct NewSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct NewSessionResponse {
    id: String,
}

impl CommandExecute for NewArgs {
    async fn run(self) -> anyhow::Result<()> {
        daemon::ensure_running().await?;
        let cwd = current_dir_string();
        let id = new_session(self.name, cwd).await?;
        println!("{id}");
        if self.open
            && let Err(e) = super::client_open::spawn_detached(&id)
        {
            eprintln!(
                "warning: failed to launch ozmux-client: {e}. \
                 Open this URL manually: {}",
                daemon_bootstrap::session_deep_link_url(&id)
            );
        }
        Ok(())
    }
}

async fn new_session(name: Option<String>, cwd: Option<String>) -> anyhow::Result<String> {
    let client = daemon::http_client(NEW_TIMEOUT)?;
    let url = format!("{}/sessions", daemon_bootstrap::HTTP_BASE_URL);
    let response = client
        .post(&url)
        .json(&NewSessionRequest { name, cwd })
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("daemon returned {status} for POST {url}: {body}");
    }

    let parsed: NewSessionResponse = response
        .json()
        .await
        .context("parse session new response body")?;
    Ok(parsed.id)
}

fn current_dir_string() -> Option<String> {
    match std::env::current_dir() {
        Ok(p) => Some(p.to_string_lossy().into_owned()),
        Err(e) => {
            eprintln!(
                "warning: could not resolve current directory: {e}; falling back to daemon CWD"
            );
            None
        }
    }
}
