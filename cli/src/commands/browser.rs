//! `ozmux browser` subcommand: opens an embedded Browser Activity in the
//! current terminal pane via the daemon's REST API.

use crate::cli::Browser;
use anyhow::Result;

/// Run the `ozmux browser` subcommand. (Implementation lands in Task 4.2.)
pub async fn run(_args: Browser) -> Result<()> {
    anyhow::bail!("ozmux browser: not yet implemented")
}
