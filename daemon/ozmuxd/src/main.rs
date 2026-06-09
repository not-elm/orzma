//! ozmuxd entrypoint: resolves the socket path, runs the daemon, and blocks
//! until shutdown is requested.
//!
//! SIGINT/SIGTERM are handled gracefully: the handler flips an atomic flag, the
//! main loop observes it and drops the `ServerHandle`, which stops accepting,
//! drains per-connection threads, shuts down the central loop, and unlinks the
//! socket. The loop also polls `ServerHandle::shutdown_requested`, the seam a
//! wire-initiated shutdown will set to exit through the same path.

use clap::Parser;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use ozmux_proto::{ClientMessage, PROTOCOL_VERSION, write_message};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

mod daemon;
mod run;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cmd = CliCommand::parse();
    cmd.command.execute().await?;
    Ok(())
}

trait CommandExecutor {
    async fn execute(self) -> anyhow::Result<()>;
}

#[derive(Debug, clap::Parser)]
struct CliCommand {
    #[subcommand]
    command: CliSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum CliSubcommand {
    Run(run::Run),
    #[subcommand]
    Daemon(daemon::Daemon),
}

impl CommandExecutor for CliSubcommand {
    async fn execute(self) -> anyhow::Result<()> {
        match self {
            Self::Run(r) => r.execute().await,
            Self::Daemon(d) => d.execute().await,
        }
    }
}
