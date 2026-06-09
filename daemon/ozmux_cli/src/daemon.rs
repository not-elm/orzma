//! `ozmux daemon` subcommand group: lifecycle management for the ozmux daemon process.

use crate::CommandExecutor;

mod start;

/// Subcommands for managing the ozmux daemon lifecycle.
#[derive(Debug, clap::Subcommand)]
pub enum Daemon {
    Start(start::Start),
}

impl CommandExecutor for Daemon {
    async fn execute(self) -> anyhow::Result<()> {
        match self {
            Self::Start(s) => s.execute().await,
        }
    }
}
