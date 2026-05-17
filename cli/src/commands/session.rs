//! `ozmux session` subcommand dispatcher. Each sibling module implements one
//! verb.

use clap::Subcommand;

use crate::commands::CommandExecute;

pub(crate) mod create;

#[derive(Subcommand)]
pub enum SessionCommand {
    /// Create a new session.
    Create(create::CreateArgs),
}

impl CommandExecute for SessionCommand {
    async fn run(self) -> anyhow::Result<()> {
        match self {
            Self::Create(args) => args.run().await,
        }
    }
}
