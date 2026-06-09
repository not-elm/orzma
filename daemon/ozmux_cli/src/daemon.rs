use crate::CommandExecutor;

mod start;
mod stop;

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
