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
///
/// `OZMUX_DAEMON_BIN` is propagated from the Tauri process environment when
/// set so the CLI can resolve a developer-built `ozmux-daemon` outside of
/// PATH; absent that, the CLI's own resolver (sibling app bundle, sibling
/// binary, or PATH) is used.
pub(crate) async fn ensure_running() -> Result<()> {
    let mut cmd = Command::new("ozmux");
    cmd.args(["daemon", "start"]);
    if let Some(v) = std::env::var_os("OZMUX_DAEMON_BIN") {
        cmd.env("OZMUX_DAEMON_BIN", v);
    }
    let status = cmd
        .status()
        .await
        .context("invoke `ozmux daemon start` (is `ozmux` on PATH?)")?;
    if !status.success() {
        anyhow::bail!("ozmux daemon failed to start (exit {:?})", status.code());
    }
    Ok(())
}
