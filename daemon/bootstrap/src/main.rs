//! `ozmux-daemon` binary entry point.
//!
//! Owns the main OS thread for CEF (`cef::initialize` → `run_message_loop` →
//! `shutdown`). Spawns a background thread that hosts a multi-thread Tokio
//! runtime running `daemon_bootstrap::serve` (axum, extensions, multiplexer,
//! terminal services). Shutdown is coordinated via a oneshot channel that the
//! bg signal handler (or a panic hook) trips; once `serve` returns, the bg
//! thread posts a quit task that releases the main thread from
//! `cef::run_message_loop`.

use anyhow::{Context as _, Result};
use cef::args::Args;
use daemon_bootstrap::{cef_lifecycle, init_tracing, serve};
use ozmux_cef_host::BrowserApp;
use ozmux_cef_host::cef_settings::{acquire_data_root, load_cef_framework};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

fn main() -> Result<()> {
    // Initialise tracing FIRST so cef::initialize and helper-process lookup
    // log lines reach stderr / daemon.log. `serve()` calls `init_tracing()`
    // again on the bg runtime; it is now idempotent via `try_init()`.
    init_tracing();

    // 1) Load CEF framework dylib (macOS) and arm api_hash. No-op elsewhere.
    load_cef_framework();

    // 2) Run CEF's helper-process dispatch hook. The bundled helpers normally
    //    execute `cef_helper`, but CefExecuteProcess still performs required
    //    browser-process startup bookkeeping before CefInitialize.
    let args = Args::new();
    dispatch_helper_process_or_continue(&args);

    // 3) Acquire data-root lock; the lock guard must outlive `run_message_loop`.
    let (browser_data_root, data_root_lock) = acquire_data_root();
    anyhow::ensure!(
        data_root_lock.is_some(),
        "another ozmux-daemon holds the browser data root {}; stop it before starting a new daemon",
        browser_data_root.display(),
    );
    let _data_root_lock = data_root_lock;

    // 4) CEF init on the main thread.
    let mut app = BrowserApp::new();
    cef_lifecycle::init_on_main(&browser_data_root, &args, &mut app)
        .context("cef::initialize on main thread failed")?;

    // 5) Shutdown coordination channel: signal handler (or panic hook) trips
    //    `stop_tx`; bg thread observes via `stop_rx` and drains `serve`.
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let stop_tx_slot = Arc::new(Mutex::new(Some(stop_tx)));

    // 6) Spawn the bg thread that owns the Tokio multi-thread runtime.
    let bg = spawn_bg_runtime(Arc::clone(&stop_tx_slot), stop_rx)?;

    // 7) Drive the CEF message loop on the main thread. Returns once the bg
    //    thread (or a panic hook) posts a quit task.
    cef_lifecycle::run_message_loop();

    // 8) Defensive: if the message loop exited for any other reason, make sure
    //    bg sees a stop signal so it does not deadlock on `stop_rx`.
    if let Some(tx) = stop_tx_slot.lock().expect("stop_tx mutex poisoned").take() {
        let _ = tx.send(());
    }

    // 9) Wait for the bg runtime to finish draining.
    let bg_result = bg.join().expect("bg thread panicked");

    // 10) Final CEF teardown after the message loop has fully exited.
    cef_lifecycle::shutdown();

    // 11) Surface any serve error.
    bg_result
}

/// Runs `cef::execute_process`. If this invocation is unexpectedly a helper
/// subprocess, exit immediately with CEF's requested code; otherwise continue as
/// the browser process.
fn dispatch_helper_process_or_continue(args: &Args) {
    let exit_code = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if exit_code >= 0 {
        std::process::exit(exit_code);
    }
}

/// Spawns the background OS thread that hosts the Tokio runtime, installs
/// SIGINT/SIGTERM and panic hooks that funnel into `stop_tx`, runs
/// `daemon_bootstrap::serve(stop_rx)`, and posts the CEF quit task on its
/// way out so the main thread's `run_message_loop` returns.
fn spawn_bg_runtime(
    stop_tx_slot: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    stop_rx: oneshot::Receiver<()>,
) -> Result<std::thread::JoinHandle<Result<()>>> {
    std::thread::Builder::new()
        .name("ozmux-daemon-tokio".into())
        .spawn(move || -> Result<()> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("ozmux-daemon-worker")
                .build()
                .context("build tokio runtime")?;

            install_panic_hook();

            let serve_result = rt.block_on(async move {
                spawn_signal_listener(Arc::clone(&stop_tx_slot));
                serve(stop_rx).await
            });

            // NOTE: posting the quit task must run *after* `serve` returns
            // (its drop order tears the axum server, registry, and CEF
            // dispatcher down) so the main thread's `run_message_loop` only
            // exits once the bg runtime is finished with CEF. If
            // `post_quit_loop` fails, the main thread keeps spinning; log and
            // continue so the result still propagates.
            if let Err(e) = ozmux_cef_host::post_command::post_quit_loop() {
                tracing::warn!(error = %e, "post_quit_loop on bg shutdown failed");
            }

            serve_result
        })
        .context("spawn ozmux-daemon-tokio thread")
}

/// Installs a process-global panic hook that posts the CEF quit task on the
/// first panic so the main thread does not get stuck in `run_message_loop`
/// after a bg-runtime panic. Subsequent panics fall through to the previous
/// hook unchanged.
fn install_panic_hook() {
    let quit_posted = Arc::new(AtomicBool::new(false));
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !quit_posted.swap(true, Ordering::AcqRel) {
            tracing::error!(?info, "bg runtime panic; posting CEF quit");
            let _ = ozmux_cef_host::post_command::post_quit_loop();
        }
        prev_hook(info);
    }));
}

/// Spawns a Tokio task that listens for SIGINT and SIGTERM and trips the
/// shutdown channel held in `stop_tx_slot` on the first signal. Must run
/// inside an active Tokio runtime.
fn spawn_signal_listener(stop_tx_slot: Arc<Mutex<Option<oneshot::Sender<()>>>>) {
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received"),
            _ = sigterm.recv() => tracing::info!("SIGTERM received"),
        }
        if let Some(tx) = stop_tx_slot.lock().expect("stop_tx mutex poisoned").take() {
            let _ = tx.send(());
        }
    });
}
