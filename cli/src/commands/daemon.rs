//! `ozmux daemon` subcommand dispatcher. Each sibling module implements one
//! verb (start/stop/status).

use clap::Subcommand;

use crate::commands::CommandExecute;

pub(crate) mod start;
pub(crate) mod status;
pub(crate) mod stop;

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon. Detaches by default; use `--foreground` to keep it
    /// attached to the current terminal (debug/development workflow).
    Start(start::StartArgs),
    /// Stop the running daemon by sending SIGTERM to the PID file owner.
    Stop(stop::StopArgs),
    /// Report whether the daemon is running and healthy.
    Status,
}

impl CommandExecute for DaemonCommand {
    async fn run(self) -> anyhow::Result<()> {
        match self {
            Self::Start(a) => a.run().await,
            Self::Stop(a) => stop::run(a).await,
            Self::Status => status::run().await,
        }
    }
}
