//! Tauri-side daemon ensure path. Delegates probe + spawn + readiness wait
//! to `ozmux daemon start`, so the launcher window only needs to invoke the
//! CLI and wait for it to exit.

use anyhow::{Context, Result};
use tokio::process::Command;

pub(crate) const DAEMON_BASE_URL: &str = "http://127.0.0.1:3200";

/// Ensures the ozmux daemon is running and `/health` is responsive by
/// invoking `ozmux daemon start`. The CLI handles probing, locking,
/// detaching, and readiness polling internally, so this function only
/// needs to wait for the wrapper to exit.
pub(crate) async fn ensure_running() -> Result<()> {
    let status = Command::new("ozmux")
        .args(["daemon", "start"])
        .status()
        .await
        .context("invoke `ozmux daemon start` (is `ozmux` on PATH?)")?;
    if !status.success() {
        anyhow::bail!("ozmux daemon failed to start (exit {:?})", status.code());
    }
    Ok(())
}
