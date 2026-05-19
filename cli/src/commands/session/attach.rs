//! `ozmux session attach` — open an existing daemon session in the Tauri
//! client. Refuses to auto-start the daemon; refuses unknown session IDs.

use anyhow::Context;
use clap::Args;
use reqwest::StatusCode;
use std::time::Duration;

use crate::commands::CommandExecute;
use crate::commands::daemon;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);

/// Arguments for the `session attach` subcommand.
#[derive(Args)]
pub(crate) struct AttachArgs {
    /// ID of the existing daemon session to open. Must be non-empty.
    #[arg(value_parser = clap::builder::NonEmptyStringValueParser::new())]
    session_id: String,
}

impl CommandExecute for AttachArgs {
    async fn run(self) -> anyhow::Result<()> {
        if !daemon::is_running() {
            anyhow::bail!(
                "daemon is not running. Run `ozmux daemon start` first."
            );
        }
        verify_session_exists(&self.session_id).await?;
        super::client_open::spawn_detached(&self.session_id)?;
        Ok(())
    }
}

async fn verify_session_exists(id: &str) -> anyhow::Result<()> {
    let client = daemon::http_client(VERIFY_TIMEOUT)?;
    let url = format!("{}/sessions/{}", daemon_bootstrap::HTTP_BASE_URL, id);
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        anyhow::bail!("session not found: {id}");
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "daemon returned {status} for GET /sessions/{id}: {body}"
        );
    }
    Ok(())
}
