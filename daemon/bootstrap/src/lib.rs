//! Daemon entry point library. Exposes `run()` (the daemon main loop that
//! was previously in `main.rs`) and the `pidfile` module. Consumers
//! (currently the `ozmux` CLI's `daemon start --foreground` command) drive
//! the tokio runtime themselves and call `run().await`.

use anyhow::{Context, bail};
use ozmux_browser::BrowserUnavailableReason;
use ozmux_browser::cef_registry::BrowserCefRegistry;
use ozmux_browser::cef_service::{CefHostSupervisor, spawn_event_pump};
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::handle::ExtensionHandles;
use ozmux_extension::registry::ExtensionRegistry;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_http_server::AppState;
use ozmux_http_server::activity_titles::ActivityTitles;
use ozmux_multiplexer::MultiplexerService;
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::signal::unix::{SignalKind, signal};

/// PID file management for the daemon process: write/read/remove plus
/// `is_process_alive` and a `PidFileGuard` RAII helper.
pub mod pidfile;

mod builtin_commands;

/// Address the daemon's HTTP server binds to.
pub const HTTP_ADDR: &str = "127.0.0.1:3200";

/// Base URL of the daemon's HTTP server.
pub const HTTP_BASE_URL: &str = "http://127.0.0.1:3200";

/// `/health` endpoint URL used by the CLI and Tauri client to confirm readiness.
pub const HEALTH_URL: &str = "http://127.0.0.1:3200/health";

/// Builds the daemon deep-link URL for the given session id, with the
/// id percent-encoded. Shared between the CLI (`session new --open`)
/// and the Tauri client launcher so the URL shape stays consistent.
pub fn session_deep_link_url(session_id: &str) -> String {
    let encoded =
        percent_encoding::utf8_percent_encode(session_id, percent_encoding::NON_ALPHANUMERIC);
    format!("{HTTP_BASE_URL}/?session={encoded}")
}

/// Returns the ozmux runtime directory (`$TMPDIR/ozmux`), creating it if
/// it does not already exist.
pub fn runtime_dir() -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("ozmux");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Runs the ozmux daemon to completion. Initialises tracing, cleans up any
/// stale PID file, loads configuration and extensions, then serves HTTP on
/// `127.0.0.1:3200` until `SIGINT` or `SIGTERM` is received.
///
/// Writes the daemon PID to `$TMPDIR/ozmux/daemon.pid` before entering the
/// serve loop and removes it on any exit path — graceful shutdown, error
/// propagation, or panic — via a `PidFileGuard` RAII helper.
pub async fn run() -> anyhow::Result<()> {
    init_tracing();
    pidfile::cleanup_if_stale()?;

    let configs = load_configs().await?;
    let runtime = init_runtime().await?;

    let registry = ExtensionRegistry::default();
    let _ext_handles = ExtensionHandles::load(&runtime, registry.clone())?;

    let terminal = TerminalService::with_runtime_root(Arc::clone(&runtime));
    let titles = ActivityTitles::default();
    let cef_host = acquire_cef_host(&runtime).await;

    let state = AppState::new(
        terminal.clone(),
        registry,
        ozmux_http_server::layout_broadcast::LayoutBroadcaster::from_env(),
        ozmux_http_server::session_broadcast::SessionBroadcaster::from_env(),
        Arc::clone(&configs),
        titles.clone(),
        cef_host,
    );

    let _event_pump = spawn_event_pump(Arc::clone(&state.cef_host), Arc::clone(&state.browser_cef));
    spawn_cef_crash_watcher(Arc::clone(&state.cef_host), Arc::clone(&state.browser_cef));
    spawn_terminal_title_bridge(terminal, titles, state.multiplexer.clone());

    let _pid_guard = pidfile::PidFileGuard::create(std::process::id())?;
    let result = run_until_shutdown(state).await;
    drop(runtime);
    result
}

/// Initialises `tracing-subscriber` with the daemon's default filter,
/// allowing `RUST_LOG` overrides. Must be called exactly once per
/// process.
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,hyper=warn,tower=warn,tokio_tungstenite=warn,tungstenite=warn",
                )
            }),
        )
        .init();
}

/// Loads the user's ozmux config; aborts daemon startup if the
/// config cannot be parsed.
async fn load_configs() -> anyhow::Result<Arc<OzmuxConfigs>> {
    match OzmuxConfigs::load().await {
        Ok(c) => {
            tracing::info!(
                prefix = ?c.shortcuts.prefix.chord,
                bindings = c.shortcuts.bindings.len(),
                "loaded ozmux config"
            );
            Ok(Arc::new(c))
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load ozmux config; aborting");
            Err(e.into())
        }
    }
}

/// Resolves a runtime root for this daemon PID and materialises both
/// the `ozmux` CLI shim and the built-in `@<name>` shims into it.
/// Shim placement is best-effort — failures log and the daemon
/// continues without the affected shims.
async fn init_runtime() -> anyhow::Result<Arc<RuntimeRoot>> {
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
    if let Err(e) = materialize_builtins(&runtime).await {
        tracing::warn!(error = %e, "failed to materialise built-in shims");
    }
    Ok(runtime)
}

/// Spawns the CEF host child and handshakes with it, returning the
/// resulting handles. On spawn error or handshake timeout, returns a
/// pre-dead handle set so the daemon comes up with the browser
/// backend disabled rather than blocking `/health`.
async fn acquire_cef_host(
    runtime: &RuntimeRoot,
) -> Arc<ozmux_browser::cef_service::CefHostHandles> {
    // NOTE: cef_host startup can hang (binary missing → no UDS connect ever;
    // missing CEF runtime libs → CefInitialize blocks). spawn_and_handshake
    // only resolves once the child sends Hello, so we cap the wait — past it
    // we proceed with the browser backend disabled rather than block the
    // entire daemon (`/health` would never come up).
    const CEF_HOST_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let socket = runtime.sock_dir().join("cef_host.sock");
    let supervisor = CefHostSupervisor::new(socket);
    let handles =
        match tokio::time::timeout(CEF_HOST_HANDSHAKE_TIMEOUT, supervisor.spawn_and_handshake())
            .await
        {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                tracing::error!(
                    error = %e,
                    "cef_host spawn_and_handshake failed; continuing with browser backend disabled"
                );
                ozmux_browser::cef_service::dead_handles_after_spawn_failure()
            }
            Err(_elapsed) => {
                tracing::error!(
                    timeout_s = CEF_HOST_HANDSHAKE_TIMEOUT.as_secs(),
                    "cef_host did not handshake in time; continuing with browser backend disabled"
                );
                ozmux_browser::cef_service::dead_handles_after_spawn_failure()
            }
        };
    Arc::new(handles)
}

/// Spawns the adapter task that bridges terminal title-change events
/// into the kind-agnostic `ActivityTitles` map so all consumers
/// (`title_republish`, WindowView builder) read from one source.
fn spawn_terminal_title_bridge(
    terminal: TerminalService,
    titles: ActivityTitles,
    multiplexer: MultiplexerService,
) {
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
                    titles.set(&wid, &aid, title.clone()).await;
                }
            }
        }
    });
}

/// Serves HTTP until `SIGINT` or `SIGTERM`, surfacing any error from
/// the serve future.
async fn run_until_shutdown(state: AppState) -> anyhow::Result<()> {
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

/// Materialises the built-in `@<name>` shims into
/// `runtime_root/bin/__builtin/`. Best-effort: returns `Err` on any
/// failure and the caller logs and proceeds without the affected
/// shims, matching the policy of `place_cli_shim()` above so the
/// CLI shim and the built-in shims have consistent behaviour on
/// error.
async fn materialize_builtins(runtime: &RuntimeRoot) -> anyhow::Result<()> {
    let ozmux_exe = std::env::current_exe().context("resolve current_exe")?;
    builtin_commands::validate_ozmux_exe(runtime.bin_dir(), &ozmux_exe).with_context(|| {
        format!(
            "ozmux_exe failed self-recursion check (path: {})",
            ozmux_exe.display()
        )
    })?;
    check_builtin_name_collision().context("an extension claims the reserved __builtin name")?;
    let bin = runtime.bin_dir().join(builtin_commands::BUILTIN_DIR_NAME);
    builtin_commands::materialize(&bin, &ozmux_exe)
        .await
        .with_context(|| format!("materialise built-in shims at {}", bin.display()))?;
    Ok(())
}

/// Scans `OZMUX_EXTENSION_ROOT` for an extension whose
/// `package.json` declares the reserved built-in dir name.
/// Returns `Err` on collision. Empty/unset env var is fine
/// (returns Ok). The pre-pass uses an opportunistic parse —
/// malformed package.json files are skipped silently because
/// `ExtensionHandles::load()` will surface them later anyway.
fn check_builtin_name_collision() -> anyhow::Result<()> {
    let Ok(root) = std::env::var("OZMUX_EXTENSION_ROOT") else {
        return Ok(());
    };
    if root.is_empty() {
        return Ok(());
    }
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let pkg_path = entry.path().join("package.json");
        let Ok(text) = std::fs::read_to_string(&pkg_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value.get("name").and_then(|n| n.as_str()) == Some(builtin_commands::BUILTIN_DIR_NAME) {
            bail!(
                "extension at {} declares reserved name {}",
                pkg_path.display(),
                builtin_commands::BUILTIN_DIR_NAME,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod lib_tests {
    use super::check_builtin_name_collision;
    use std::fs;

    fn with_extension_root<F: FnOnce()>(root: &std::path::Path, f: F) {
        // NOTE: process-global env mutation. Other tests in this binary
        // that touch OZMUX_EXTENSION_ROOT must be either serial-guarded
        // or in a separate test binary.
        unsafe { std::env::set_var("OZMUX_EXTENSION_ROOT", root) };
        f();
        unsafe { std::env::remove_var("OZMUX_EXTENSION_ROOT") };
    }

    #[test]
    fn collision_check_passes_when_no_offending_extension() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("memo");
        fs::create_dir_all(&ext).unwrap();
        fs::write(
            ext.join("package.json"),
            r#"{"name":"memo","main":"bootstrap.ts"}"#,
        )
        .unwrap();
        with_extension_root(dir.path(), || {
            assert!(check_builtin_name_collision().is_ok());
        });
    }

    #[test]
    fn collision_check_errors_on_reserved_name() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("usurper");
        fs::create_dir_all(&ext).unwrap();
        fs::write(
            ext.join("package.json"),
            r#"{"name":"__builtin","main":"x.ts"}"#,
        )
        .unwrap();
        with_extension_root(dir.path(), || {
            assert!(check_builtin_name_collision().is_err());
        });
    }

    #[test]
    fn collision_check_tolerates_malformed_package_json() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("broken");
        fs::create_dir_all(&ext).unwrap();
        fs::write(ext.join("package.json"), "not json").unwrap();
        with_extension_root(dir.path(), || {
            assert!(check_builtin_name_collision().is_ok());
        });
    }

    #[test]
    fn collision_check_tolerates_missing_env_var() {
        // No with_extension_root wrapper; env var stays unset.
        unsafe { std::env::remove_var("OZMUX_EXTENSION_ROOT") };
        assert!(check_builtin_name_collision().is_ok());
    }
}
