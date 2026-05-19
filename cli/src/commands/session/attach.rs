//! `ozmux session attach` — open an existing daemon session in the Tauri
//! client. Refuses to auto-start the daemon; refuses unknown session IDs.

use clap::Args;

use crate::commands::CommandExecute;

/// Arguments for the `session attach` subcommand.
#[derive(Args)]
pub(crate) struct AttachArgs {
    /// ID of the existing daemon session to open. Must be non-empty.
    #[arg(value_parser = clap::builder::NonEmptyStringValueParser::new())]
    session_id: String,
}

impl CommandExecute for AttachArgs {
    async fn run(self) -> anyhow::Result<()> {
        let _ = self.session_id;
        anyhow::bail!("ozmux session attach: not yet implemented")
    }
}
