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

use ozmux_browser::BrowserUnavailableReason;
use ozmux_browser::cef_registry::BrowserCefRegistry;
use ozmux_browser::cef_service::{CefHostSupervisor, spawn_event_pump};
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::handle::ExtensionHandles;
use ozmux_extension::registry::ExtensionRegistry;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_http_server::AppState;
use ozmux_http_server::activity_titles::ActivityTitles;
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
    if let Err(e) = place_cli_shim(&runtime) {
        tracing::warn!(error = %e, "failed to place ozmux CLI shim");
    }

    let registry = ExtensionRegistry::default();
    let _ext_handles = ExtensionHandles::load(&runtime, registry.clone())?;

    let terminal = TerminalService::with_runtime_root(Arc::clone(&runtime));
    let titles = ActivityTitles::default();

    let cef_host_socket = runtime.sock_dir().join("cef_host.sock");
    let supervisor = CefHostSupervisor::new(cef_host_socket);
    let cef_host_handles = match supervisor.spawn_and_handshake().await {
        Ok(handles) => handles,
        Err(e) => {
            // NOTE: a missing/broken cef_host (CI without CEF runtime deps, dev
            // box that never ran `make bundle-cef-host`, hostile sandbox) must
            // NOT block daemon startup — terminal and extension activities are
            // independent of the browser path. Fall back to a pre-`is_dead`
            // handle so every Browser Activity request short-circuits with
            // `BrowserUnavailable` via the existing checks.
            tracing::error!(
                error = %e,
                "cef_host spawn_and_handshake failed; continuing with browser backend disabled"
            );
            ozmux_browser::cef_service::dead_handles_after_spawn_failure()
        }
    };
    let cef_host = Arc::new(cef_host_handles);

    let state = AppState::new(
        terminal.clone(),
        registry,
        ozmux_http_server::layout_broadcast::LayoutBroadcaster::from_env(),
        ozmux_http_server::session_broadcast::SessionBroadcaster::from_env(),
        Arc::clone(&configs),
        titles.clone(),
        cef_host,
    );

    // Drain HostEvents from cef_host and route NavStateChanged / TitleChanged
    // into per-activity watch::Sender<NavState> on the BrowserCefRegistry so
    // that the cef WS handler can push BrowserServerMsg::Nav to subscribers.
    let _event_pump = spawn_event_pump(Arc::clone(&state.cef_host), Arc::clone(&state.browser_cef));

    // Crash-watcher: awaits cef_host exit and broadcasts BrowserUnavailable to
    // all connected WS clients. No respawn — Plan 3 territory.
    spawn_cef_crash_watcher(Arc::clone(&state.cef_host), Arc::clone(&state.browser_cef));

    // Adapter task: bridge terminal title-change notifications into ActivityTitles
    // so that all consumers (title_republish, WindowView builder) read from the
    // kind-agnostic map rather than directly from TerminalService.
    let terminal_titles = titles.clone();
    let multiplexer = state.multiplexer.clone();
    tokio::spawn(async move {
        let mut rx = terminal.subscribe_title_changes();
        loop {
            let wid = match rx.recv().await {
                Ok(wid) => wid,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            // Collect all activity ids in this window that have a terminal kind.
            // The multiplexer lock is held only for the brief id-collection step.
            let aids: Vec<ozmux_multiplexer::ActivityId> = multiplexer
                .with_window(&wid, |w| {
                    w.panes
                        .iter()
                        .flat_map(|(_, p)| p.activities.iter())
                        .filter(|a| matches!(a.kind, ozmux_multiplexer::ActivityKind::Terminal))
                        .map(|a| a.id.clone())
                        .collect()
                })
                .await
                .unwrap_or_default();
            // Snapshot current titles from TerminalService and push into ActivityTitles.
            let all = terminal.all_titles().await;
            for aid in aids {
                if let Some(title) = all.get(&aid) {
                    terminal_titles.set(&wid, &aid, title.clone()).await;
                }
            }
        }
    });

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

/// Spawns a task that watches for unexpected `cef_host` exit and notifies
/// all connected WS clients via `BrowserCefRegistry::broadcast_unavailable`.
///
/// On exit the task sets `CefHostHandles::is_dead` (Release) then broadcasts
/// `BrowserUnavailableReason::RetryExhausted` carrying the process status.
/// No respawn is attempted — that is Plan 3 territory.
fn spawn_cef_crash_watcher(
    cef_host: Arc<ozmux_browser::cef_service::CefHostHandles>,
    registry: Arc<BrowserCefRegistry>,
) {
    let Some(mut child) = cef_host.take_child() else {
        tracing::warn!("cef_host child already taken; crash-watcher not started");
        return;
    };
    let is_dead = cef_host.is_dead_handle();
    tokio::spawn(async move {
        let status = child.wait().await;
        is_dead.store(true, Ordering::Release);
        let last_error = match &status {
            Ok(s) => format!("cef_host exited: {s:?}"),
            Err(e) => format!("cef_host wait error: {e}"),
        };
        tracing::error!(status = ?status, "cef_host exited unexpectedly");
        registry.broadcast_unavailable(BrowserUnavailableReason::RetryExhausted { last_error });
    });
}

/// Place the `ozmux` CLI binary at `runtime/bin/ozmux/ozmux` so PTY-spawned
/// shells can invoke it directly via PATH. Best-effort: logs a warning and
/// skips if the binary cannot be found.
fn place_cli_shim(runtime: &RuntimeRoot) -> std::io::Result<()> {
    let me = std::env::current_exe()?;
    let Some(parent) = me.parent() else {
        tracing::warn!("self exe has no parent dir; skipping ozmux CLI shim");
        return Ok(());
    };
    // NOTE: the CLI binary is named `ozmux` (from cli/Cargo.toml's [[bin]] name).
    let cli_src = parent.join("ozmux");
    if !cli_src.exists() {
        tracing::warn!(
            path = %cli_src.display(),
            "ozmux CLI binary not found next to bootstrap; `ozmux browser` will not be on PATH"
        );
        return Ok(());
    }
    let shim_dir = runtime.root().join("bin").join("ozmux");
    std::fs::create_dir_all(&shim_dir)?;
    let shim = shim_dir.join("ozmux");
    let _ = std::fs::remove_file(&shim);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&cli_src, &shim)?;
    #[cfg(not(unix))]
    std::fs::copy(&cli_src, &shim)?;
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
