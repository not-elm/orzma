use std::sync::Arc;

use ozmux_extension::commands::ExtensionCommands;
use ozmux_http_server::AppState;
use ozmux_session::{SessionState, WindowStore};
use ozmux_terminal::TerminalService;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("ozmux_http_server=info,warn")
            }),
        )
        .init();

    let commands = ExtensionCommands::load().await?;
    let wrappers = Arc::new(commands.materialize_wrappers()?);

    let state = AppState {
        sessions: SessionState::default(),
        windows: WindowStore::default(),
        terminal: TerminalService::with_extension_wrappers(wrappers),
    };

    ozmux_extension::host::serve(state.sessions.clone());
    ozmux_http_server::serve(state).await?;
    Ok(())
}
