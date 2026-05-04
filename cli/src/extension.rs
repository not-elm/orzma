use crate::CommandExecutor;
use interprocess::local_socket::{ConnectOptions, GenericFilePath, ToFsName};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(clap::Args, Debug)]
pub struct ExtensionCommand {
    pub command: String,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
    pub args: Vec<String>,
}

impl CommandExecutor for ExtensionCommand {
    async fn execute(self) -> anyhow::Result<()> {
        println!("{self:?}");
        let socket_path = std::env::temp_dir().join("ozmux-extension-host.sock");
        let name = socket_path.to_fs_name::<GenericFilePath>()?;
        let mut conn = ConnectOptions::new().name(name).connect_tokio().await?;

        conn.write_all(br#"{"type":"status"}"#).await?;
        conn.write_all(b"\n").await?;

        let mut reader = BufReader::new(conn);

        let mut line = String::new();
        reader.read_line(&mut line).await?;
        Ok(())
    }
}
