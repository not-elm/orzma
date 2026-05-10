use ozmux_extension::handle::ExtensionHandles;
use ozmux_extension::registry::ExtensionRegistry;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_http_server::AppState;
use ozmux_multiplexer::MultiplexerService;
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,hyper=warn,tower=warn,tokio_tungstenite=warn,tungstenite=warn",
                )
            }),
        )
        .init();

    let parent = std::env::temp_dir().join("ozmux");
    std::fs::create_dir_all(&parent)?;
    RuntimeRoot::gc_stale(&parent)?;
    let longest = longest_extension_name()?;
    let runtime = Arc::new(RuntimeRoot::resolve_in(
        &parent,
        std::process::id(),
        &longest,
    )?);

    let _ext_handles = ExtensionHandles::load(&runtime, ExtensionRegistry::default())?;

    let state = AppState {
        multiplexer: Arc::new(Mutex::new(MultiplexerService::default())),
        terminal: TerminalService::with_runtime_root(Arc::clone(&runtime)),
    };

    let serve = ozmux_http_server::serve(state);
    tokio::select! {
        result = serve => result?,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received SIGINT, shutting down");
        }
    }
    drop(runtime);
    Ok(())
}

fn longest_extension_name() -> std::io::Result<String> {
    let Ok(root) = std::env::var("OZMUX_EXTENSION_ROOT") else {
        return Ok("x".to_string());
    };
    if root.is_empty() {
        return Ok("x".to_string());
    }
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("OZMUX_EXTENSION_ROOT={root} does not exist; ignoring");
            return Ok("x".to_string());
        }
        Err(e) => return Err(e),
    };
    let mut longest = String::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(s) = name.to_str() else { continue };
        if s.len() > longest.len() {
            longest = s.to_string();
        }
    }
    if longest.is_empty() {
        longest = "x".to_string();
    }
    Ok(longest)
}
