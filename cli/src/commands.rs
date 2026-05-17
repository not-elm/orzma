//! Top-level CLI subcommand modules.

pub(crate) mod daemon;
pub(crate) mod session;

pub trait CommandExecute {
    async fn run(self) -> anyhow::Result<()>;
}
