//! `ozmux session` subcommand dispatcher. Each sibling module implements one
//! verb.

use clap::Subcommand;

use crate::commands::CommandExecute;

pub(crate) mod attach;
pub(crate) mod client_open;
pub(crate) mod new;

#[derive(Subcommand)]
pub enum SessionCommand {
    /// Create a new session.
    New(new::NewArgs),
    /// Open an existing session in the Tauri client window.
    Attach(attach::AttachArgs),
}

impl CommandExecute for SessionCommand {
    async fn run(self) -> anyhow::Result<()> {
        match self {
            Self::New(args) => args.run().await,
            Self::Attach(args) => args.run().await,
        }
    }
}
