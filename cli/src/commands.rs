//! Top-level CLI subcommand modules.

pub mod browser;
pub(crate) mod daemon;
pub(crate) mod session;

pub trait CommandExecute {
    async fn run(self) -> anyhow::Result<()>;
}
