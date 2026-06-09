//! `ozmux run` subcommand: runs the daemon in the foreground.

use crate::CommandExecutor;

/// Starts the ozmux server in the foreground, blocking until shutdown.
#[derive(Debug, clap::Args)]
pub struct Run {}

impl CommandExecutor for Run {
    async fn execute(self) -> anyhow::Result<()> {
        todo!("ozmux run: implemented in Task A4")
    }
}
