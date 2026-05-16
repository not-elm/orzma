//! `ozmux daemon` subcommand dispatcher. Each sibling module implements one
//! verb (start/stop/status).

use clap::{Args, Subcommand};

pub(crate) mod start;
pub(crate) mod status;
pub(crate) mod stop;

#[derive(Args)]
pub(crate) struct DaemonArgs {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the daemon. Detaches by default; use `--foreground` to keep it
    /// attached to the current terminal (debug/development workflow).
    Start(start::StartArgs),
    /// Stop the running daemon by sending SIGTERM to the PID file owner.
    Stop(stop::StopArgs),
    /// Report whether the daemon is running and healthy.
    Status,
}

pub(crate) async fn run(args: DaemonArgs) -> anyhow::Result<()> {
    match args.command {
        DaemonCommand::Start(a) => start::run(a).await,
        DaemonCommand::Stop(a) => stop::run(a).await,
        DaemonCommand::Status => status::run().await,
    }
}
