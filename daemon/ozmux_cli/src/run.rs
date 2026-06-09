use crate::CommandExecutor;

#[derive(Debug, clap::Args)]
pub struct Run {}

impl CommandExecutor for Run {
    async fn execute(self) -> anyhow::Result<()> {
        todo!("ozmux runの実装. OzmuxServerを使ってozmux-serverを起動する")
    }
}
