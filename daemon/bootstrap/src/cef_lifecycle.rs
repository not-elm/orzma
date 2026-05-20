//! CEF initialize / shutdown helpers for `ozmux-daemon`. Runs on the main
//! thread. Plan 3.

use anyhow::{Context as _, Result};
use cef::App;
use cef::args::Args;
use ozmux_cef_host::cef_settings::build_cef_settings;
use std::path::Path;
use tokio::sync::oneshot;

/// Initialise CEF on the calling (main) thread. Must be called before any
/// `cef::run_message_loop()` or browser create. Plan 3 R1 finding: cef-rs
/// auto-installs the macOS `NSApplication` subclass via the CEF C++ layer,
/// so no manual `objc2` step is needed.
pub fn init_on_main(browser_data_root: &Path, app: &mut App) -> Result<()> {
    let settings = build_cef_settings(browser_data_root);
    let main_args = Args::new();
    let ok = cef::initialize(
        Some(main_args.as_main_args()),
        Some(&settings),
        Some(app),
        std::ptr::null_mut(),
    );
    anyhow::ensure!(ok == 1, "cef::initialize returned {ok} (expected 1)");
    tracing::info!("cef::initialize succeeded");
    Ok(())
}

/// Runs the CEF message loop on the calling (main) thread. Blocks until
/// `post_quit_loop` is invoked. Returns once the loop exits.
pub fn run_message_loop() {
    tracing::info!("entering cef::run_message_loop");
    cef::run_message_loop();
    tracing::info!("cef::run_message_loop returned");
}

/// Drains CEF: signals shutdown phase 1 to the bg runtime (via `stop_tx`),
/// waits for the bg runtime to acknowledge completion (via `complete_rx`),
/// then posts the quit task to release the main thread from
/// `run_message_loop`.
///
/// Caller layout:
/// ```text
/// 1. Bg thread: receive shutdown signal → drain axum → close all browsers
///    via post_task → ack via complete_tx.
/// 2. Main thread: await complete_rx (blocking via `blocking_recv`).
/// 3. Main thread: post_quit_loop() → run_message_loop returns.
/// ```
pub fn shutdown_sequence(
    stop_tx: oneshot::Sender<()>,
    complete_rx: oneshot::Receiver<()>,
) -> Result<()> {
    // Phase 1: tell bg to drain.
    if stop_tx.send(()).is_err() {
        tracing::warn!("bg shutdown channel dropped before signal; bg may have already exited");
    }

    // Phase 2: wait for bg to confirm drain + browser teardown complete.
    // NOTE: we are on the main thread with no tokio reactor available, so
    // block synchronously via `blocking_recv`.
    let drain_result = complete_rx.blocking_recv();
    if let Err(e) = drain_result {
        tracing::warn!(error = %e, "bg never sent shutdown-complete; quitting message loop anyway");
    }

    // Phase 3: tell CEF to quit.
    ozmux_cef_host::post_command::post_quit_loop()
        .context("post_quit_loop failed; CEF may have already shut down")?;
    Ok(())
}

/// Final CEF teardown. Must be called after `run_message_loop` returns
/// and after the bg tokio runtime has been dropped.
pub fn shutdown() {
    tracing::info!("cef::shutdown");
    cef::shutdown();
}
