//! Daemon entry point library. Exposes `run()` (the daemon main loop that
//! was previously in `main.rs`) and the `pidfile` module. Consumers
//! (currently the `ozmux` CLI's `daemon start --foreground` command) drive
//! the tokio runtime themselves and call `run().await`.

use anyhow::Context;
use ozmux_browser::cef_registry::BrowserCefRegistry;
use ozmux_browser::cef_service::spawn_event_pump;
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::handle::ExtensionHandles;
use ozmux_extension::registry::ExtensionRegistry;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_http_server::AppState;
use ozmux_http_server::activity_titles::ActivityTitles;
use ozmux_multiplexer::MultiplexerService;
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};

/// PID file management for the daemon process: write/read/remove plus
/// `is_process_alive` and a `PidFileGuard` RAII helper.
pub mod pidfile;

/// CEF initialize / shutdown helpers invoked by the daemon main thread.
pub mod cef_lifecycle;

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

/// RAII bundle that owns daemon subsystem handles (runtime root, extension
/// child processes, event pump task, PID file guard). Dropping the bundle
/// tears every subsystem down in the right order: background tasks abort
/// first, then `PidFileGuard`'s `Drop` removes the PID file, then the runtime
/// root cleans up its scratch directory.
pub struct RuntimeHandles {
    event_pump: tokio::task::JoinHandle<()>,
    _ext_handles: ExtensionHandles,
    _pid_guard: pidfile::PidFileGuard,
    _runtime: Arc<RuntimeRoot>,
}

impl Drop for RuntimeHandles {
    fn drop(&mut self) {
        // NOTE: abort the background tasks first so they cannot race with
        // PidFileGuard / RuntimeRoot teardown by trying to touch state that
        // is about to disappear.
        self.event_pump.abort();
    }
}

/// Builds the daemon's `AppState` together with a `RuntimeHandles` bundle.
///
/// Performs every startup step that does not require an active HTTP serve
/// loop: stale PID cleanup, config + runtime root initialisation, extension
/// discovery, terminal/title services, CEF host acquisition, AppState
/// construction, plus the event pump, CEF crash watcher, terminal title
/// bridge, and PID file guard. Does **not** initialise tracing — callers
/// (`serve` and the deprecated `run`) own that side effect.
pub async fn build_state() -> anyhow::Result<(AppState, RuntimeHandles)> {
    pidfile::cleanup_if_stale()?;

    let configs = load_configs().await?;
    let runtime = init_runtime().await?;

    let registry = ExtensionRegistry::default();
    let ext_handles = ExtensionHandles::load(&runtime, registry.clone())?;

    let terminal = TerminalService::with_runtime_root(Arc::clone(&runtime));
    let titles = ActivityTitles::default();
    let browser_cef = Arc::new(BrowserCefRegistry::new());
    let cef_dispatcher =
        acquire_cef_host(&runtime, browser_cef.session_id(), registry.clone()).await;

    let state = AppState::new(
        terminal.clone(),
        registry,
        ozmux_http_server::layout_broadcast::LayoutBroadcaster::from_env(),
        ozmux_http_server::session_broadcast::SessionBroadcaster::from_env(),
        Arc::clone(&configs),
        titles.clone(),
        Arc::clone(&cef_dispatcher),
        Arc::clone(&browser_cef),
    );

    let event_pump = spawn_event_pump(Arc::clone(&cef_dispatcher), Arc::clone(&state.browser_cef));
    spawn_terminal_title_bridge(terminal, titles, state.multiplexer.clone());

    let pid_guard = pidfile::PidFileGuard::create(std::process::id())?;
    Ok((
        state,
        RuntimeHandles {
            event_pump,
            _ext_handles: ext_handles,
            _pid_guard: pid_guard,
            _runtime: runtime,
        },
    ))
}

/// Serves the ozmux daemon HTTP API until `stop_rx` fires or the serve
/// future returns. Initialises tracing, calls `build_state`, and runs the
/// axum server. Signal handling is the caller's responsibility — pass a
/// `oneshot::Receiver` whose sender is tripped by whatever shutdown
/// orchestration the host (CEF-aware main, integration test, …) uses.
pub async fn serve(stop_rx: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    init_tracing();
    let (state, _handles) = build_state().await?;
    let serve = ozmux_http_server::serve(state);
    tokio::select! {
        result = serve => result?,
        _ = stop_rx => {
            tracing::info!("serve: stop signal received");
        }
    }
    Ok(())
}

/// Runs the ozmux daemon to completion using built-in `SIGINT`/`SIGTERM`
/// signal handling. Initialises tracing, builds the state bundle, and
/// serves HTTP on `127.0.0.1:3200` until a signal arrives.
///
/// Writes the daemon PID to `$TMPDIR/ozmux/daemon.pid` for the lifetime of
/// the call and removes it on any exit path — graceful shutdown, error
/// propagation, or panic — via the `RuntimeHandles` bundle's RAII drop.
#[deprecated(note = "Use `serve(stop_rx)` from a CEF-aware main with explicit signal handling")]
pub async fn run() -> anyhow::Result<()> {
    init_tracing();
    let (state, _handles) = build_state().await?;
    run_until_shutdown(state).await
}

/// Initialises `tracing-subscriber` with the daemon's default filter,
/// allowing `RUST_LOG` overrides. Idempotent — subsequent calls are
/// no-ops (the first installer wins).
///
/// Public so that `ozmux-daemon::main` can install tracing BEFORE
/// `cef::initialize`, ensuring CEF-init log lines (framework paths,
/// subprocess path, success/failure) reach stderr / daemon.log.
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,hyper=warn,tower=warn,tokio_tungstenite=warn,tungstenite=warn",
                )
            }),
        )
        .try_init();
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

/// Builds the in-process CEF dispatcher. CEF itself is initialised on the
/// main thread by `cef_lifecycle::init_on_main` before the bg runtime spawns
/// this function; here we only construct the daemon-side wrappers (`BrowserPool`,
/// `PoolHandle`, and the host-event channel) and hand them to
/// `LiveCefDispatcher`.
///
/// The pool starts empty; the first `HostCommand::BrowserCreate` populates it
/// via the in-process create_browser path (Plan 3 Task 12).
async fn acquire_cef_host(
    _runtime: &RuntimeRoot,
    session_id: u64,
    extensions: ExtensionRegistry,
) -> Arc<dyn ozmux_browser::cef_dispatcher::CefDispatcher> {
    use ozmux_browser_cef_protocol::wire::HostEvent;
    use ozmux_cef_host::FrameBufferPool;
    use ozmux_cef_host::pool::BrowserPool;
    use ozmux_cef_host::post_command::PoolHandle;
    use ozmux_cef_host::profile;
    use tokio::sync::mpsc;

    // CEF handlers want an UnboundedSender (they fire from non-async callbacks
    // on the CEF UI / IO threads where awaiting backpressure is not possible).
    // The CefDispatcher trait surfaces a bounded mpsc::Receiver, so a tiny
    // forwarder task bridges the two.
    let (unb_tx, mut unb_rx) = mpsc::unbounded_channel::<HostEvent>();
    let (bnd_tx, bnd_rx) = mpsc::channel::<HostEvent>(256);
    tokio::spawn(async move {
        while let Some(ev) = unb_rx.recv().await {
            if bnd_tx.send(ev).await.is_err() {
                break;
            }
        }
    });

    let browser_data_root = profile::browser_data_root();
    // 60-frame budget matches the FrameRing budget in `frame_ring.rs`; under
    // sustained 60 fps × 33 MB (4K BGRA) load this caps in-flight pool
    // allocations at ~2 GB before the recycler stabilises.
    let frame_pool = Arc::new(FrameBufferPool::new(60));
    // Persistent disk profiles are paused pool-wide (see BrowserPool docs); the
    // flag is preserved so the lock plumbing stays in place for future work.
    let pool = BrowserPool::new(
        unb_tx,
        browser_data_root,
        false,
        session_id,
        frame_pool,
        extensions.clone(),
    );
    let pool_handle = PoolHandle::new(pool);

    let dispatcher = Arc::new(ozmux_browser::cef_dispatcher::live::LiveCefDispatcher::new(
        pool_handle.clone(),
        bnd_rx,
    ));

    // Install the V8↔extension bridge. Must happen after `PoolHandle::new`
    // plants its back-reference: the bridge holds a `PoolHandle` clone to
    // post `DispatchExtensionResponse` commands from its UDS reader.
    //
    // The unavailable callback holds a `Weak<LiveCefDispatcher>` so the
    // bridge does not keep the dispatcher alive after daemon shutdown.
    let dispatcher_weak = Arc::downgrade(&dispatcher);
    let unavailable_cb: ozmux_cef_host::extension_bridge::UnavailableCallback =
        Arc::new(move |aid, reason| {
            if let Some(d) = dispatcher_weak.upgrade() {
                d.mark_unavailable(Some(aid), reason);
            }
        });
    let bridge = ozmux_cef_host::extension_bridge::ExtensionBridge::new(
        tokio::runtime::Handle::current(),
        extensions,
        pool_handle.clone(),
    )
    .with_unavailable_callback(unavailable_cb);
    pool_handle.install_bridge(bridge);

    dispatcher as Arc<dyn ozmux_browser::cef_dispatcher::CefDispatcher>
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

/// Place the `ozmux` CLI binary at `runtime/bin/ozmux/ozmux` so PTY-spawned
/// shells can invoke it directly via PATH. Resolution uses a 4-level
/// fallback:
/// 1. `OZMUX_CLI_BIN` env override
/// 2. macOS-only: if the current executable lives inside an
///    `*.app/Contents/MacOS/` bundle, climb 4 parents up (out of the app
///    bundle and its parent dir) and look for a sibling `ozmux`
/// 3. Sibling `ozmux` next to the current executable
/// 4. `which::which("ozmux")` against `PATH`
///
/// Best-effort: logs a warning and skips if no candidate is found.
fn place_cli_shim(runtime: &RuntimeRoot) -> std::io::Result<()> {
    let Some(cli_src) = resolve_ozmux_cli() else {
        tracing::warn!(
            "ozmux CLI binary could not be resolved; `ozmux browser` will not be on PATH"
        );
        return Ok(());
    };
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

/// Resolves the path to the `ozmux` CLI binary using the layered fallback
/// described on `place_cli_shim`. Returns `None` when no candidate exists.
fn resolve_ozmux_cli() -> Option<std::path::PathBuf> {
    if let Some(v) = std::env::var_os("OZMUX_CLI_BIN") {
        let p = std::path::PathBuf::from(v);
        if p.is_file() {
            return Some(p);
        }
    }
    let me = std::env::current_exe().ok()?;
    #[cfg(target_os = "macos")]
    if let Some(p) = resolve_ozmux_cli_from_app_bundle(&me) {
        return Some(p);
    }
    if let Some(parent) = me.parent() {
        let sibling = parent.join("ozmux");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    which::which("ozmux").ok()
}

/// macOS-only resolver step: when `me` lives inside an `*.app/Contents/MacOS/`
/// bundle, climb up four ancestors (`MacOS` → `Contents` → `*.app` →
/// containing dir) and probe for a sibling `ozmux` binary next to the bundle.
/// This matches the layout produced by `make dev` (the `.app` and `ozmux`
/// land side-by-side in `$CARGO_BIN_DIR`) and the dev tree (`target/debug/`
/// holds both `ozmux-daemon.app` and `ozmux`).
#[cfg(target_os = "macos")]
fn resolve_ozmux_cli_from_app_bundle(me: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut ancestor = me;
    for _ in 0..3 {
        ancestor = ancestor.parent()?;
    }
    let extension = ancestor.extension()?.to_str()?;
    if extension != "app" {
        return None;
    }
    let sibling = ancestor.parent()?.join("ozmux");
    sibling.is_file().then_some(sibling)
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
    // NOTE: shims must exec the `ozmux` CLI, not `current_exe()` — since the
    // daemon is its own binary, `current_exe()` points at `ozmux-daemon` and
    // would make `@browser` start a second daemon (lock contention panic).
    let Some(ozmux_exe) = resolve_ozmux_cli() else {
        tracing::warn!(
            "ozmux CLI binary could not be resolved; built-in @-shims (e.g. @browser) will not be on PATH"
        );
        return Ok(());
    };
    builtin_commands::validate_ozmux_exe(runtime.bin_dir(), &ozmux_exe).with_context(|| {
        format!(
            "ozmux_exe failed self-recursion check (path: {})",
            ozmux_exe.display()
        )
    })?;
    let bin = runtime.bin_dir().join(builtin_commands::BUILTIN_DIR_NAME);
    builtin_commands::materialize(&bin, &ozmux_exe)
        .await
        .with_context(|| format!("materialise built-in shims at {}", bin.display()))?;
    Ok(())
}
