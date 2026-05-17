//! `ozmux daemon` subcommand dispatcher. Each sibling module implements one
//! verb (start/stop/status), plus the shared `ensure_running` helper.

use anyhow::Context;
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
            Self::Start(args) => args.run().await,
            Self::Stop(args) => args.run().await,
            Self::Status => status::run().await,
        }
    }
}

/// Whether `ensure_running` found the daemon already up or started it.
pub(crate) enum DaemonStartOutcome {
    AlreadyRunning,
    Started,
}

/// Ensures the ozmux daemon is running, spawning it detached if it is not.
///
/// Writes nothing to stdout or stderr on the success path, so callers with
/// their own stdout contract (e.g. `session create`) get a clean stream.
/// Spawn-failure diagnostics may still reach stderr.
pub(crate) async fn ensure_running() -> anyhow::Result<DaemonStartOutcome> {
    if start::is_running() {
        return Ok(DaemonStartOutcome::AlreadyRunning);
    }

    let lock = start::acquire_lock()
        .await
        .context("acquire daemon launcher lock")?;

    if start::is_running() {
        drop(lock);
        return Ok(DaemonStartOutcome::AlreadyRunning);
    }

    start::spawn_detached().context("spawn ozmux daemon")?;
    start::wait_until_ready()
        .await
        .context("daemon did not become ready in time")?;
    drop(lock);

    Ok(DaemonStartOutcome::Started)
}
