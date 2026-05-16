//! Daemon entry point. Sets up the runtime root, loads extensions, builds
//! `AppState`, and runs the HTTP server until SIGINT.

use ozmux_browser::BrowserService;
use ozmux_browser::cef_service::{CefHostSupervisor, spawn_event_pump};
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::handle::ExtensionHandles;
use ozmux_extension::registry::ExtensionRegistry;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_http_server::AppState;
use ozmux_http_server::activity_titles::ActivityTitles;
use ozmux_terminal::TerminalService;
use std::sync::Arc;

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

    let parent = std::env::temp_dir().join("ozmux");
    std::fs::create_dir_all(&parent)?;
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

    let browser = BrowserService::new(Arc::clone(&runtime));
    let terminal = TerminalService::with_runtime_root(Arc::clone(&runtime));
    let titles = ActivityTitles::default();

    let cef_host_socket = runtime.sock_dir().join("cef_host.sock");
    let supervisor = CefHostSupervisor::new(cef_host_socket);
    let cef_host_handles = supervisor.spawn_and_handshake().await.map_err(|e| {
        tracing::error!(error = %e, "cef_host spawn_and_handshake failed");
        e
    })?;
    let cef_host = Arc::new(cef_host_handles);

    let state = AppState::new(
        browser,
        terminal.clone(),
        registry,
        ozmux_http_server::layout_broadcast::LayoutBroadcaster::from_env(),
        Arc::clone(&configs),
        titles.clone(),
        cef_host,
    );

    // Drain HostEvents from cef_host and route NavStateChanged / TitleChanged
    // into per-activity watch::Sender<NavState> on the BrowserCefRegistry so
    // that the cef WS handler can push BrowserServerMsg::Nav to subscribers.
    let _event_pump = spawn_event_pump(Arc::clone(&state.cef_host), Arc::clone(&state.browser_cef));

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

    // Adapter task: bridge browser title-change notifications into ActivityTitles
    // so the tab bar shows live page titles without polling.
    let browser_titles = titles.clone();
    let browser_svc = state.browser.clone();
    tokio::spawn(async move {
        let mut rx = browser_svc.subscribe_title_changes();
        loop {
            match rx.recv().await {
                Ok((wid, aid, title)) => {
                    browser_titles.set(&wid, &aid, title).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });

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
