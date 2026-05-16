//! Daemon entry point library. Exposes `run()` (the daemon main loop that
//! was previously in `main.rs`) and the `pidfile` module. Consumers
//! (currently the `ozmux` CLI's `daemon start --foreground` command) drive
//! the tokio runtime themselves and call `run().await`.

/// PID file management for the daemon process: write/read/remove plus
/// `is_process_alive` and a `PidFileGuard` RAII helper.
pub mod pidfile;

/// Address the daemon's HTTP server binds to.
pub const HTTP_ADDR: &str = "127.0.0.1:3200";

/// Base URL of the daemon's HTTP server.
pub const HTTP_BASE_URL: &str = "http://127.0.0.1:3200";

/// `/health` endpoint URL used by the CLI and Tauri client to confirm readiness.
pub const HEALTH_URL: &str = "http://127.0.0.1:3200/health";

/// Returns the ozmux runtime directory (`$TMPDIR/ozmux`), creating it if
/// it does not already exist.
pub fn runtime_dir() -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("ozmux");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

use ozmux_configs::OzmuxConfigs;
use ozmux_extension::handle::ExtensionHandles;
use ozmux_extension::registry::ExtensionRegistry;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_http_server::AppState;
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};

/// Runs the ozmux daemon to completion. Initialises tracing, cleans up any
/// stale PID file, loads configuration and extensions, then serves HTTP on
/// `127.0.0.1:3200` until `SIGINT` or `SIGTERM` is received.
///
/// Writes the daemon PID to `$TMPDIR/ozmux/daemon.pid` before entering the
/// serve loop and removes it on any exit path — graceful shutdown, error
/// propagation, or panic — via a `PidFileGuard` RAII helper.
pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,hyper=warn,tower=warn,tokio_tungstenite=warn,tungstenite=warn",
                )
            }),
        )
        .init();

    pidfile::cleanup_if_stale()?;

    let configs = match OzmuxConfigs::load().await {
        Ok(c) => {
            tracing::info!(
                prefix = ?c.shortcuts.prefix.chord,
                bindings = c.shortcuts.bindings.len(),
                "loaded ozmux config"
            );
            Arc::new(c)
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load ozmux config; aborting");
            return Err(e.into());
        }
    };

    let parent = runtime_dir()?;
    RuntimeRoot::gc_stale(&parent)?;
    let longest = longest_extension_name()?;
    let runtime = Arc::new(RuntimeRoot::resolve_in(
        &parent,
        std::process::id(),
        &longest,
    )?);

    let registry = ExtensionRegistry::default();
    let _ext_handles = ExtensionHandles::load(&runtime, registry.clone())?;

    let state = AppState::new(
        TerminalService::with_runtime_root(Arc::clone(&runtime)),
        registry,
        ozmux_http_server::layout_broadcast::LayoutBroadcaster::from_env(),
        ozmux_http_server::session_broadcast::SessionBroadcaster::from_env(),
        Arc::clone(&configs),
    );

    let _pid_guard = pidfile::PidFileGuard::create(std::process::id())?;

    let mut sigterm = signal(SignalKind::terminate())?;
    let serve = ozmux_http_server::serve(state);
    tokio::select! {
        result = serve => result?,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("received SIGTERM, shutting down");
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
