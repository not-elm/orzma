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
}

#[derive(Serialize)]
struct NewSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Deserialize)]
struct NewSessionResponse {
    id: String,
}

impl CommandExecute for NewArgs {
    async fn run(self) -> anyhow::Result<()> {
        daemon::ensure_running().await?;
        let id = new_session(self.name).await?;
        println!("{id}");
        Ok(())
    }
}

async fn new_session(name: Option<String>) -> anyhow::Result<String> {
    let client = daemon::http_client(NEW_TIMEOUT)?;
    let url = format!("{}/sessions", daemon_bootstrap::HTTP_BASE_URL);
    let response = client
        .post(&url)
        .json(&NewSessionRequest { name })
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
