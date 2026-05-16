//! `ozmux daemon stop` — signal the running daemon to shut down.

use clap::Args;

#[derive(Args)]
pub struct StopArgs {
    /// If the daemon does not exit within 10s of SIGTERM, escalate to
    /// SIGKILL and remove the PID file.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(_args: StopArgs) -> anyhow::Result<()> {
    anyhow::bail!("ozmux daemon stop: not yet implemented")
}
