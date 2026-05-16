//! Top-level CLI subcommand modules.

pub(crate) mod daemon;

pub trait CommandExecute {
    async fn run(self) -> anyhow::Result<()>;
}
